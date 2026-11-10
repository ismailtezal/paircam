// A small C layer over FFmpeg so the Rust side needs no generated bindings.
//
// It decodes H.264 or HEVC access units (length-prefixed NAL units, as the phone's encoder emits
// them) into NV12 pictures in system memory, on the GPU through D3D11VA when available, and
// converts NV12 to BGRA at any size for the preview.

#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavutil/error.h>
#include <libavutil/hwcontext.h>
#include <libavutil/imgutils.h>
#include <libavutil/log.h>
#include <libavutil/pixfmt.h>
#include <libswscale/swscale.h>

typedef struct PcFrame {
    int32_t width;
    int32_t height;
    int64_t pts;
    const uint8_t *y;
    int32_t y_stride;
    const uint8_t *uv;
    int32_t uv_stride;
} PcFrame;

typedef struct PcDecoder {
    AVCodecContext *context;
    AVBufferRef *hw_device;
    AVPacket *packet;
    AVFrame *decoded;
    AVFrame *transferred;
    AVFrame *nv12;
    struct SwsContext *to_nv12;
    int hardware;
} PcDecoder;

static void set_error(char *error, int error_len, const char *what, int code) {
    if (error == NULL || error_len <= 0) {
        return;
    }
    char detail[AV_ERROR_MAX_STRING_SIZE] = {0};
    if (code != 0) {
        av_strerror(code, detail, sizeof(detail));
        snprintf(error, (size_t)error_len, "%s: %s", what, detail);
    } else {
        snprintf(error, (size_t)error_len, "%s", what);
    }
}

static enum AVPixelFormat pick_format(AVCodecContext *context, const enum AVPixelFormat *formats) {
    for (const enum AVPixelFormat *format = formats; *format != AV_PIX_FMT_NONE; format++) {
        if (*format == AV_PIX_FMT_D3D11) {
            return *format;
        }
    }
    // The GPU cannot decode this stream; fall back to the first software format offered.
    ((PcDecoder *)context->opaque)->hardware = 0;
    return formats[0];
}

void pc_decoder_close(PcDecoder *decoder) {
    if (decoder == NULL) {
        return;
    }
    sws_freeContext(decoder->to_nv12);
    av_frame_free(&decoder->nv12);
    av_frame_free(&decoder->transferred);
    av_frame_free(&decoder->decoded);
    av_packet_free(&decoder->packet);
    avcodec_free_context(&decoder->context);
    av_buffer_unref(&decoder->hw_device);
    av_free(decoder);
}

// codec: 0 = H.264, 1 = HEVC. `record` is the avcC or hvcC decoder configuration record.
PcDecoder *pc_decoder_open(int codec, const uint8_t *record, int record_len, int use_hardware,
                           char *error, int error_len) {
    av_log_set_level(AV_LOG_ERROR);

    const AVCodec *implementation =
        avcodec_find_decoder(codec == 1 ? AV_CODEC_ID_HEVC : AV_CODEC_ID_H264);
    if (implementation == NULL) {
        set_error(error, error_len, "this FFmpeg build has no decoder for the codec", 0);
        return NULL;
    }

    PcDecoder *decoder = av_mallocz(sizeof(PcDecoder));
    if (decoder == NULL) {
        set_error(error, error_len, "out of memory", 0);
        return NULL;
    }

    decoder->context = avcodec_alloc_context3(implementation);
    decoder->packet = av_packet_alloc();
    decoder->decoded = av_frame_alloc();
    decoder->transferred = av_frame_alloc();
    decoder->nv12 = av_frame_alloc();
    if (!decoder->context || !decoder->packet || !decoder->decoded || !decoder->transferred ||
        !decoder->nv12) {
        set_error(error, error_len, "out of memory", 0);
        pc_decoder_close(decoder);
        return NULL;
    }

    decoder->context->opaque = decoder;
    decoder->context->flags |= AV_CODEC_FLAG_LOW_DELAY;
    decoder->context->extradata = av_mallocz((size_t)record_len + AV_INPUT_BUFFER_PADDING_SIZE);
    if (decoder->context->extradata == NULL) {
        set_error(error, error_len, "out of memory", 0);
        pc_decoder_close(decoder);
        return NULL;
    }
    memcpy(decoder->context->extradata, record, (size_t)record_len);
    decoder->context->extradata_size = record_len;

    if (use_hardware &&
        av_hwdevice_ctx_create(&decoder->hw_device, AV_HWDEVICE_TYPE_D3D11VA, NULL, NULL, 0) == 0) {
        decoder->context->hw_device_ctx = av_buffer_ref(decoder->hw_device);
        decoder->context->get_format = pick_format;
        decoder->hardware = 1;
    } else {
        // Software decoding: one picture in, one picture out, with no frame-thread delay.
        decoder->context->thread_type = FF_THREAD_SLICE;
    }

    int result = avcodec_open2(decoder->context, implementation, NULL);
    if (result < 0) {
        set_error(error, error_len, "cannot open the decoder", result);
        pc_decoder_close(decoder);
        return NULL;
    }
    return decoder;
}

int pc_decoder_is_hardware(const PcDecoder *decoder) {
    return decoder->hardware;
}

static int to_nv12(PcDecoder *decoder, AVFrame *picture, PcFrame *out) {
    if (picture->format == AV_PIX_FMT_NV12) {
        out->y = picture->data[0];
        out->y_stride = picture->linesize[0];
        out->uv = picture->data[1];
        out->uv_stride = picture->linesize[1];
        return 0;
    }

    AVFrame *nv12 = decoder->nv12;
    if (nv12->width != picture->width || nv12->height != picture->height) {
        av_frame_unref(nv12);
        nv12->format = AV_PIX_FMT_NV12;
        nv12->width = picture->width;
        nv12->height = picture->height;
        int result = av_frame_get_buffer(nv12, 0);
        if (result < 0) {
            return result;
        }
    }

    decoder->to_nv12 = sws_getCachedContext(decoder->to_nv12, picture->width, picture->height,
                                            (enum AVPixelFormat)picture->format, picture->width,
                                            picture->height, AV_PIX_FMT_NV12, SWS_FAST_BILINEAR,
                                            NULL, NULL, NULL);
    if (decoder->to_nv12 == NULL) {
        return AVERROR(EINVAL);
    }
    sws_scale(decoder->to_nv12, (const uint8_t *const *)picture->data, picture->linesize, 0,
              picture->height, nv12->data, nv12->linesize);

    out->y = nv12->data[0];
    out->y_stride = nv12->linesize[0];
    out->uv = nv12->data[1];
    out->uv_stride = nv12->linesize[1];
    return 0;
}

// Decodes one access unit. Returns 1 when `out` holds a picture, 0 when the decoder needs more
// data, and a negative FFmpeg error code on failure. The planes in `out` stay valid until the
// next call on this decoder.
int pc_decoder_decode(PcDecoder *decoder, const uint8_t *data, int len, int64_t pts, PcFrame *out) {
    int result = av_new_packet(decoder->packet, len);
    if (result < 0) {
        return result;
    }
    memcpy(decoder->packet->data, data, (size_t)len);
    decoder->packet->pts = pts;

    result = avcodec_send_packet(decoder->context, decoder->packet);
    av_packet_unref(decoder->packet);
    if (result < 0) {
        return result;
    }

    av_frame_unref(decoder->decoded);
    result = avcodec_receive_frame(decoder->context, decoder->decoded);
    if (result == AVERROR(EAGAIN)) {
        return 0;
    }
    if (result < 0) {
        return result;
    }

    AVFrame *picture = decoder->decoded;
    if (picture->format == AV_PIX_FMT_D3D11) {
        av_frame_unref(decoder->transferred);
        result = av_hwframe_transfer_data(decoder->transferred, picture, 0);
        if (result < 0) {
            return result;
        }
        picture = decoder->transferred;
    }

    out->width = decoder->decoded->width;
    out->height = decoder->decoded->height;
    out->pts = decoder->decoded->pts;
    result = to_nv12(decoder, picture, out);
    return result < 0 ? result : 1;
}

void pc_error_string(int code, char *error, int error_len) {
    av_strerror(code, error, (size_t)error_len);
}

// ---------------------------------------------------------------------------------------------
// NV12 to BGRA, for the preview
// ---------------------------------------------------------------------------------------------

typedef struct PcScaler {
    struct SwsContext *context;
} PcScaler;

PcScaler *pc_scaler_new(void) {
    return av_mallocz(sizeof(PcScaler));
}

void pc_scaler_free(PcScaler *scaler) {
    if (scaler == NULL) {
        return;
    }
    sws_freeContext(scaler->context);
    av_free(scaler);
}

// Converts and scales an NV12 picture into a caller-provided BGRA buffer. Returns 0 on success.
int pc_scaler_nv12_to_bgra(PcScaler *scaler, const PcFrame *frame, uint8_t *bgra, int bgra_width,
                           int bgra_height, int bgra_stride) {
    scaler->context = sws_getCachedContext(scaler->context, frame->width, frame->height,
                                           AV_PIX_FMT_NV12, bgra_width, bgra_height,
                                           AV_PIX_FMT_BGRA, SWS_BILINEAR, NULL, NULL, NULL);
    if (scaler->context == NULL) {
        return AVERROR(EINVAL);
    }

    const uint8_t *const source[4] = {frame->y, frame->uv, NULL, NULL};
    const int source_stride[4] = {frame->y_stride, frame->uv_stride, 0, 0};
    uint8_t *const target[4] = {bgra, NULL, NULL, NULL};
    const int target_stride[4] = {bgra_stride, 0, 0, 0};
    sws_scale(scaler->context, source, source_stride, 0, frame->height, target, target_stride);
    return 0;
}
