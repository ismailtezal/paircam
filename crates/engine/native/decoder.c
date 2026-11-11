// A small C layer over FFmpeg so the Rust side needs no generated bindings.
//
// It decodes H.264 or HEVC access units (length-prefixed NAL units, as the phone's encoder emits
// them) into NV12 pictures in system memory, on the GPU through D3D11VA when available, and
// converts NV12 to BGRA at any size for the preview. pc_transform_nv12 crops, scales, turns,
// mirrors and flips NV12 pictures for the camera feed and the preview.

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
    // Working memory for pc_transform_nv12: a picture before it is turned, and the index maps
    // that turn it.
    uint8_t *scratch;
    size_t scratch_size;
} PcScaler;

PcScaler *pc_scaler_new(void) {
    return av_mallocz(sizeof(PcScaler));
}

void pc_scaler_free(PcScaler *scaler) {
    if (scaler == NULL) {
        return;
    }
    sws_freeContext(scaler->context);
    av_free(scaler->scratch);
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

// ---------------------------------------------------------------------------------------------
// NV12 to NV12 at another size, for the virtual camera
// ---------------------------------------------------------------------------------------------

// Scales an NV12 picture into caller-provided NV12 planes of `width` x `height`. The target
// planes may sit inside a larger picture (for letterboxing). Use a scaler of its own for this:
// alternating with nv12_to_bgra on one scaler would rebuild the context every call.
// Returns 0 on success.
int pc_scaler_nv12_to_nv12(PcScaler *scaler, const PcFrame *frame, uint8_t *y, int y_stride,
                           uint8_t *uv, int uv_stride, int width, int height) {
    scaler->context = sws_getCachedContext(scaler->context, frame->width, frame->height,
                                           AV_PIX_FMT_NV12, width, height, AV_PIX_FMT_NV12,
                                           SWS_BICUBIC, NULL, NULL, NULL);
    if (scaler->context == NULL) {
        return AVERROR(EINVAL);
    }

    const uint8_t *const source[4] = {frame->y, frame->uv, NULL, NULL};
    const int source_stride[4] = {frame->y_stride, frame->uv_stride, 0, 0};
    uint8_t *const target[4] = {y, uv, NULL, NULL};
    const int target_stride[4] = {y_stride, uv_stride, 0, 0};
    sws_scale(scaler->context, source, source_stride, 0, frame->height, target, target_stride);
    return 0;
}

// ---------------------------------------------------------------------------------------------
// The picture transform: crop, scale, rotate, mirror and flip in one pass, for the camera feed
// and the preview alike
// ---------------------------------------------------------------------------------------------

// Mirrors engine/src/decoder.rs `RawTransform`. Every coordinate and size is even.
typedef struct PcTransform {
    // The part of the source to use, in source pixels.
    int32_t crop_x;
    int32_t crop_y;
    int32_t crop_width;
    int32_t crop_height;
    // Clockwise degrees: 0, 90, 180 or 270. Applied first, then the mirror, then the flip.
    int32_t rotation;
    int32_t mirror;
    int32_t flip;
    // Where the picture lands in the target. The rest of the target becomes black.
    int32_t content_x;
    int32_t content_y;
    int32_t content_width;
    int32_t content_height;
} PcTransform;

// Edge length of the square tiles a rotation is copied in, so both the rows read and the rows
// written stay in the cache.
#define PC_TILE 64

static int is_even(int32_t value) {
    return (value & 1) == 0;
}

static int ensure_scratch(PcScaler *scaler, size_t size) {
    if (scaler->scratch_size >= size) {
        return 0;
    }
    av_free(scaler->scratch);
    scaler->scratch = av_malloc(size);
    scaler->scratch_size = scaler->scratch == NULL ? 0 : size;
    return scaler->scratch == NULL ? AVERROR(ENOMEM) : 0;
}

// Limited-range black: luma 16, neutral chroma.
static void fill_black(uint8_t *y, int y_stride, uint8_t *uv, int uv_stride, int width,
                       int height) {
    for (int row = 0; row < height; row++) {
        memset(y + (ptrdiff_t)row * y_stride, 16, (size_t)width);
    }
    for (int row = 0; row < height / 2; row++) {
        memset(uv + (ptrdiff_t)row * uv_stride, 128, (size_t)width);
    }
}

// Scales with the legacy context, which FFmpeg rebuilds cheaply when the crop changes size
// (the dynamic sws_scale_frame mode rebuilds a whole thread pool, over 100 ms). Enlarging uses
// bicubic for sharpness; shrinking uses bilinear, which swscale widens to the scale factor so
// it still averages every source pixel, at a fraction of bicubic's cost.
static int scale_nv12(PcScaler *scaler, const uint8_t *source_y, int source_y_stride,
                      const uint8_t *source_uv, int source_uv_stride, int source_width,
                      int source_height, uint8_t *y, int y_stride, uint8_t *uv, int uv_stride,
                      int width, int height) {
    int enlarging = width > source_width || height > source_height;
    scaler->context = sws_getCachedContext(
        scaler->context, source_width, source_height, AV_PIX_FMT_NV12, width, height,
        AV_PIX_FMT_NV12, enlarging ? SWS_BICUBIC : SWS_BILINEAR, NULL, NULL, NULL);
    if (scaler->context == NULL) {
        return AVERROR(EINVAL);
    }
    const uint8_t *const source[4] = {source_y, source_uv, NULL, NULL};
    const int source_stride[4] = {source_y_stride, source_uv_stride, 0, 0};
    uint8_t *const target[4] = {y, uv, NULL, NULL};
    const int target_stride[4] = {y_stride, uv_stride, 0, 0};
    sws_scale(scaler->context, source, source_stride, 0, source_height, target, target_stride);
    return 0;
}

// Reverses the four 2-byte pairs in a 64-bit block.
static uint64_t reverse_pairs(uint64_t value) {
    value = ((value & 0x0000FFFF0000FFFFull) << 16) | ((value >> 16) & 0x0000FFFF0000FFFFull);
    return (value << 32) | (value >> 32);
}

// Reverses the eight bytes in a 64-bit block.
static uint64_t reverse_bytes(uint64_t value) {
    value = ((value & 0x00FF00FF00FF00FFull) << 8) | ((value >> 8) & 0x00FF00FF00FF00FFull);
    return reverse_pairs(value);
}

// Reverses the order of `count` elements of `size` bytes (1 or 2) in every row, in place,
// eight bytes at a time from both ends.
static void mirror_rows(uint8_t *plane, int stride, int rows, int count, int size) {
    int bytes = count * size;
    for (int row = 0; row < rows; row++) {
        uint8_t *line = plane + (ptrdiff_t)row * stride;
        int left = 0;
        int right = bytes;
        while (right - left >= 16) {
            uint64_t head;
            uint64_t tail;
            memcpy(&head, line + left, 8);
            memcpy(&tail, line + right - 8, 8);
            head = size == 1 ? reverse_bytes(head) : reverse_pairs(head);
            tail = size == 1 ? reverse_bytes(tail) : reverse_pairs(tail);
            memcpy(line + left, &tail, 8);
            memcpy(line + right - 8, &head, 8);
            left += 8;
            right -= 8;
        }
        // Fewer than sixteen bytes remain in the middle.
        for (int first = left, last = right - size; first < last; first += size, last -= size) {
            for (int byte = 0; byte < size; byte++) {
                uint8_t swap = line[first + byte];
                line[first + byte] = line[last + byte];
                line[last + byte] = swap;
            }
        }
    }
}

// Swaps the rows of a plane top to bottom, in place, through a buffer of `row_bytes`.
static void flip_rows(uint8_t *plane, int stride, int rows, int row_bytes, uint8_t *buffer) {
    for (int top = 0, bottom = rows - 1; top < bottom; top++, bottom--) {
        uint8_t *upper = plane + (ptrdiff_t)top * stride;
        uint8_t *lower = plane + (ptrdiff_t)bottom * stride;
        memcpy(buffer, upper, (size_t)row_bytes);
        memcpy(upper, lower, (size_t)row_bytes);
        memcpy(lower, buffer, (size_t)row_bytes);
    }
}

// target(x, y) = source(column columns[y], row rows[x]) for elements of `size` bytes (1 for
// luma, 2 for an interleaved chroma pair). This is a transposition with reordered indices,
// which covers both quarter turns with any mirror and flip.
static void remap(const uint8_t *source, int source_stride, uint8_t *target, int target_stride,
                  int width, int height, const int32_t *rows, const int32_t *columns, int size) {
    for (int tile_y = 0; tile_y < height; tile_y += PC_TILE) {
        int end_y = tile_y + PC_TILE < height ? tile_y + PC_TILE : height;
        for (int tile_x = 0; tile_x < width; tile_x += PC_TILE) {
            int end_x = tile_x + PC_TILE < width ? tile_x + PC_TILE : width;
            for (int y = tile_y; y < end_y; y++) {
                uint8_t *line = target + (ptrdiff_t)y * target_stride;
                const uint8_t *column = source + (ptrdiff_t)columns[y] * size;
                if (size == 1) {
                    for (int x = tile_x; x < end_x; x++) {
                        line[x] = column[(ptrdiff_t)rows[x] * source_stride];
                    }
                } else {
                    for (int x = tile_x; x < end_x; x++) {
                        const uint8_t *pair = column + (ptrdiff_t)rows[x] * source_stride;
                        line[2 * x] = pair[0];
                        line[2 * x + 1] = pair[1];
                    }
                }
            }
        }
    }
}

// Fills the index maps that turn a quarter-turned picture of `width` x `height` (the size after
// turning) with the mirror and flip applied on top.
static void quarter_turn_maps(int rotation, int mirror, int flip, int width, int height,
                              int32_t *rows, int32_t *columns) {
    for (int x = 0; x < width; x++) {
        int oriented = mirror ? width - 1 - x : x;
        rows[x] = rotation == 90 ? width - 1 - oriented : oriented;
    }
    for (int y = 0; y < height; y++) {
        int oriented = flip ? height - 1 - y : y;
        columns[y] = rotation == 90 ? oriented : height - 1 - oriented;
    }
}

// Turns the `crop` of `frame` into the target picture as `transform` describes. The target is
// `width` x `height` NV12; outside the content rectangle it becomes black. Returns 0 on success.
int pc_transform_nv12(PcScaler *scaler, const PcFrame *frame, const PcTransform *transform,
                      uint8_t *y, int y_stride, uint8_t *uv, int uv_stride, int width,
                      int height) {
    const PcTransform *t = transform;
    int rotation = t->rotation;
    int mirror = t->mirror != 0;
    int flip = t->flip != 0;
    if (rotation == 180) {
        // A half turn is a mirror and a flip together.
        rotation = 0;
        mirror = !mirror;
        flip = !flip;
    }

    int valid = (rotation == 0 || rotation == 90 || rotation == 270) && is_even(t->crop_x) &&
                is_even(t->crop_y) && is_even(t->crop_width) && is_even(t->crop_height) &&
                is_even(t->content_x) && is_even(t->content_y) && is_even(t->content_width) &&
                is_even(t->content_height) && t->crop_x >= 0 && t->crop_y >= 0 &&
                t->crop_width >= 2 && t->crop_height >= 2 &&
                t->crop_x + t->crop_width <= frame->width &&
                t->crop_y + t->crop_height <= frame->height && t->content_x >= 0 &&
                t->content_y >= 0 && t->content_width >= 2 && t->content_height >= 2 &&
                t->content_x + t->content_width <= width &&
                t->content_y + t->content_height <= height;
    if (!valid) {
        return AVERROR(EINVAL);
    }

    if (t->content_x != 0 || t->content_y != 0 || t->content_width != width ||
        t->content_height != height) {
        fill_black(y, y_stride, uv, uv_stride, width, height);
    }

    const uint8_t *source_y =
        frame->y + (ptrdiff_t)t->crop_y * frame->y_stride + t->crop_x;
    const uint8_t *source_uv =
        frame->uv + (ptrdiff_t)(t->crop_y / 2) * frame->uv_stride + t->crop_x;
    uint8_t *content_y = y + (ptrdiff_t)t->content_y * y_stride + t->content_x;
    uint8_t *content_uv = uv + (ptrdiff_t)(t->content_y / 2) * uv_stride + t->content_x;
    int content_width = t->content_width;
    int content_height = t->content_height;

    int result;
    if (rotation == 0) {
        result = scale_nv12(scaler, source_y, frame->y_stride, source_uv,
                                frame->uv_stride, t->crop_width, t->crop_height, content_y,
                                y_stride, content_uv, uv_stride, content_width, content_height);
        if (result < 0) {
            return result;
        }
        if (mirror) {
            mirror_rows(content_y, y_stride, content_height, content_width, 1);
            mirror_rows(content_uv, uv_stride, content_height / 2, content_width / 2, 2);
        }
        if (flip) {
            result = ensure_scratch(scaler, (size_t)content_width);
            if (result < 0) {
                return result;
            }
            flip_rows(content_y, y_stride, content_height, content_width, scaler->scratch);
            flip_rows(content_uv, uv_stride, content_height / 2, content_width,
                      scaler->scratch);
        }
        return 0;
    }

    // A quarter turn: scale into the scratch picture at the turned size, then turn it into
    // place with the mirror and flip folded into the index maps.
    int turned_width = content_height;
    int turned_height = content_width;
    size_t luma_size = (size_t)turned_width * (size_t)turned_height;
    size_t chroma_size = luma_size / 2;
    // The maps start on a 16-byte boundary after the picture.
    size_t maps_offset = (luma_size + chroma_size + 15) & ~(size_t)15;
    size_t maps_size = sizeof(int32_t) * (size_t)(content_width + content_height);
    result = ensure_scratch(scaler, maps_offset + maps_size);
    if (result < 0) {
        return result;
    }
    uint8_t *turned_y = scaler->scratch;
    uint8_t *turned_uv = turned_y + luma_size;
    int32_t *rows = (int32_t *)(scaler->scratch + maps_offset);
    int32_t *columns = rows + content_width;

    result = scale_nv12(scaler, source_y, frame->y_stride, source_uv, frame->uv_stride,
                        t->crop_width, t->crop_height, turned_y, turned_width, turned_uv,
                        turned_width, turned_width, turned_height);
    if (result < 0) {
        return result;
    }

    quarter_turn_maps(rotation, mirror, flip, content_width, content_height, rows, columns);
    remap(turned_y, turned_width, content_y, y_stride, content_width, content_height, rows,
          columns, 1);
    quarter_turn_maps(rotation, mirror, flip, content_width / 2, content_height / 2, rows,
                      columns);
    remap(turned_uv, turned_width, content_uv, uv_stride, content_width / 2, content_height / 2,
          rows, columns, 2);
    return 0;
}
