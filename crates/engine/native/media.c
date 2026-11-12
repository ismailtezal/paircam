// Reads a video or still picture file for the away screen: every picture comes out as NV12 in
// system memory, with the time it should be shown. Sound is ignored.

#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/display.h>
#include <libavutil/error.h>
#include <libavutil/mathematics.h>
#include <libavutil/pixdesc.h>
#include <libavutil/pixfmt.h>
#include <libswscale/swscale.h>

#include "frame.h"

// Larger pictures than this are refused, so a broken or hostile file cannot ask for gigabytes.
#define PC_MEDIA_MAX_SIDE 8192

typedef struct PcMedia {
    AVFormatContext *format;
    AVCodecContext *context;
    AVPacket *packet;
    AVFrame *decoded;
    AVFrame *nv12;
    struct SwsContext *to_nv12;
    int stream;
    // Clockwise quarter turns the file asks for before it is shown: 0, 90, 180 or 270.
    int rotation;
    // The timestamp of the first picture, in the stream's time base.
    int64_t first_pts;
    // Pictures handed out since the start or the last rewind, for files without timestamps.
    int64_t count;
    int draining;
} PcMedia;

static void media_error(char *error, int error_len, const char *what, int code) {
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

void pc_media_close(PcMedia *media) {
    if (media == NULL) {
        return;
    }
    sws_freeContext(media->to_nv12);
    av_frame_free(&media->nv12);
    av_frame_free(&media->decoded);
    av_packet_free(&media->packet);
    avcodec_free_context(&media->context);
    avformat_close_input(&media->format);
    av_free(media);
}

// The clockwise quarter turn a stream's display matrix asks for.
static int stream_rotation(const AVStream *stream) {
    const AVPacketSideData *side =
        av_packet_side_data_get(stream->codecpar->coded_side_data,
                                stream->codecpar->nb_coded_side_data, AV_PKT_DATA_DISPLAYMATRIX);
    if (side == NULL || side->size < 9 * sizeof(int32_t)) {
        return 0;
    }
    // The matrix gives the counter-clockwise angle; showing the picture upright takes the
    // opposite turn.
    double clockwise = -av_display_rotation_get((const int32_t *)side->data);
    if (clockwise != clockwise) {
        return 0;
    }
    int quarter = (int)((clockwise + 360.0 * 4.0 + 45.0) / 90.0) % 4;
    return quarter * 90;
}

// Opens the file at `path` (UTF-8). Returns NULL and fills `error` when it cannot be played.
PcMedia *pc_media_open(const char *path, char *error, int error_len) {
    av_log_set_level(AV_LOG_ERROR);

    PcMedia *media = av_mallocz(sizeof(PcMedia));
    if (media == NULL) {
        media_error(error, error_len, "out of memory", 0);
        return NULL;
    }
    media->first_pts = AV_NOPTS_VALUE;

    int result = avformat_open_input(&media->format, path, NULL, NULL);
    if (result < 0) {
        media_error(error, error_len, "cannot open the file", result);
        pc_media_close(media);
        return NULL;
    }
    result = avformat_find_stream_info(media->format, NULL);
    if (result < 0) {
        media_error(error, error_len, "cannot read the file", result);
        pc_media_close(media);
        return NULL;
    }

    const AVCodec *implementation = NULL;
    result = av_find_best_stream(media->format, AVMEDIA_TYPE_VIDEO, -1, -1, &implementation, 0);
    if (result < 0 || implementation == NULL) {
        media_error(error, error_len, "the file has no picture this app can decode", 0);
        pc_media_close(media);
        return NULL;
    }
    media->stream = result;
    for (unsigned int index = 0; index < media->format->nb_streams; index++) {
        if ((int)index != media->stream) {
            media->format->streams[index]->discard = AVDISCARD_ALL;
        }
    }
    const AVStream *stream = media->format->streams[media->stream];
    media->rotation = stream_rotation(stream);

    media->context = avcodec_alloc_context3(implementation);
    media->packet = av_packet_alloc();
    media->decoded = av_frame_alloc();
    media->nv12 = av_frame_alloc();
    if (!media->context || !media->packet || !media->decoded || !media->nv12) {
        media_error(error, error_len, "out of memory", 0);
        pc_media_close(media);
        return NULL;
    }
    result = avcodec_parameters_to_context(media->context, stream->codecpar);
    if (result < 0) {
        media_error(error, error_len, "cannot read the picture format", result);
        pc_media_close(media);
        return NULL;
    }
    if (media->context->width > PC_MEDIA_MAX_SIDE || media->context->height > PC_MEDIA_MAX_SIDE) {
        media_error(error, error_len, "the picture is too large", 0);
        pc_media_close(media);
        return NULL;
    }
    // Let FFmpeg pick the thread count: an away screen can afford a few frames of delay.
    media->context->thread_count = 0;
    result = avcodec_open2(media->context, implementation, NULL);
    if (result < 0) {
        media_error(error, error_len, "cannot open the decoder", result);
        pc_media_close(media);
        return NULL;
    }
    return media;
}

int pc_media_rotation(const PcMedia *media) {
    return media->rotation;
}

static int is_full_range(const AVFrame *picture) {
    if (picture->color_range == AVCOL_RANGE_JPEG) {
        return 1;
    }
    switch (picture->format) {
    case AV_PIX_FMT_YUVJ420P:
    case AV_PIX_FMT_YUVJ422P:
    case AV_PIX_FMT_YUVJ444P:
    case AV_PIX_FMT_YUVJ440P:
    case AV_PIX_FMT_YUVJ411P:
        return 1;
    default:
        return 0;
    }
}

// Converts a decoded picture to video-range NV12 with even sides.
static int media_to_nv12(PcMedia *media, const AVFrame *picture, PcFrame *out) {
    int width = picture->width & ~1;
    int height = picture->height & ~1;
    if (width < 2 || height < 2 || picture->width > PC_MEDIA_MAX_SIDE ||
        picture->height > PC_MEDIA_MAX_SIDE) {
        return AVERROR(EINVAL);
    }

    AVFrame *nv12 = media->nv12;
    if (nv12->width != width || nv12->height != height) {
        av_frame_unref(nv12);
        nv12->format = AV_PIX_FMT_NV12;
        nv12->width = width;
        nv12->height = height;
        int result = av_frame_get_buffer(nv12, 0);
        if (result < 0) {
            return result;
        }
    }

    media->to_nv12 = sws_getCachedContext(media->to_nv12, picture->width, picture->height,
                                          (enum AVPixelFormat)picture->format, width, height,
                                          AV_PIX_FMT_NV12, SWS_BILINEAR, NULL, NULL, NULL);
    if (media->to_nv12 == NULL) {
        return AVERROR(EINVAL);
    }
    // The camera feed is BT.709 in video range. Photos and some clips are full range.
    int source_space = picture->colorspace == AVCOL_SPC_UNSPECIFIED
                           ? (picture->height >= 720 ? SWS_CS_ITU709 : SWS_CS_ITU601)
                           : picture->colorspace;
    const int *source = sws_getCoefficients(source_space);
    const int *target = sws_getCoefficients(SWS_CS_ITU709);
    sws_setColorspaceDetails(media->to_nv12, source, is_full_range(picture), target, 0, 0, 1 << 16,
                             1 << 16);
    sws_scale(media->to_nv12, (const uint8_t *const *)picture->data, picture->linesize, 0,
              picture->height, nv12->data, nv12->linesize);

    out->width = width;
    out->height = height;
    out->y = nv12->data[0];
    out->y_stride = nv12->linesize[0];
    out->uv = nv12->data[1];
    out->uv_stride = nv12->linesize[1];
    return 0;
}

// When the picture should be shown, in microseconds after the first one.
static int64_t media_time(PcMedia *media, const AVFrame *picture) {
    const AVStream *stream = media->format->streams[media->stream];
    int64_t pts = picture->best_effort_timestamp;
    if (pts == AV_NOPTS_VALUE) {
        AVRational rate = stream->avg_frame_rate;
        if (rate.num <= 0 || rate.den <= 0) {
            rate = (AVRational){25, 1};
        }
        return av_rescale(media->count, (int64_t)rate.den * 1000000, rate.num);
    }
    if (media->first_pts == AV_NOPTS_VALUE) {
        media->first_pts = pts;
    }
    int64_t since = av_rescale_q(pts - media->first_pts, stream->time_base, AV_TIME_BASE_Q);
    return since < 0 ? 0 : since;
}

// Decodes the next picture. Returns 1 when `out` holds it (`out->pts` is its time in
// microseconds after the first picture), 0 at the end of the file, and a negative FFmpeg error
// code on failure. The planes in `out` stay valid until the next call on this file.
int pc_media_next(PcMedia *media, PcFrame *out) {
    for (;;) {
        av_frame_unref(media->decoded);
        int result = avcodec_receive_frame(media->context, media->decoded);
        if (result == 0) {
            out->pts = media_time(media, media->decoded);
            media->count++;
            result = media_to_nv12(media, media->decoded, out);
            return result < 0 ? result : 1;
        }
        if (result == AVERROR_EOF) {
            return 0;
        }
        if (result != AVERROR(EAGAIN)) {
            return result;
        }
        if (media->draining) {
            return 0;
        }

        result = av_read_frame(media->format, media->packet);
        if (result == AVERROR_EOF) {
            media->draining = 1;
            result = avcodec_send_packet(media->context, NULL);
            if (result < 0 && result != AVERROR_EOF) {
                return result;
            }
            continue;
        }
        if (result < 0) {
            return result;
        }
        if (media->packet->stream_index == media->stream) {
            result = avcodec_send_packet(media->context, media->packet);
        } else {
            result = 0;
        }
        av_packet_unref(media->packet);
        // A damaged packet is skipped; the decoder recovers at the next keyframe.
        if (result < 0 && result != AVERROR_INVALIDDATA && result != AVERROR(EAGAIN)) {
            return result;
        }
    }
}

// Goes back to the first picture. Returns 0 on success; on failure the caller reopens the file.
int pc_media_rewind(PcMedia *media) {
    const AVStream *stream = media->format->streams[media->stream];
    int64_t start = stream->start_time == AV_NOPTS_VALUE ? 0 : stream->start_time;
    int result = av_seek_frame(media->format, media->stream, start, AVSEEK_FLAG_BACKWARD);
    if (result < 0) {
        return result;
    }
    avcodec_flush_buffers(media->context);
    media->draining = 0;
    media->count = 0;
    return 0;
}
