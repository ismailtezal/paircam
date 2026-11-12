// The picture type shared by the C files and mirrored by `RawFrame` in engine/src/decoder.rs.

#ifndef PAIRCAM_FRAME_H
#define PAIRCAM_FRAME_H

#include <stdint.h>

// An NV12 picture: a luma plane and an interleaved chroma plane, each with its own stride.
typedef struct PcFrame {
    int32_t width;
    int32_t height;
    int64_t pts;
    const uint8_t *y;
    int32_t y_stride;
    const uint8_t *uv;
    int32_t uv_stride;
} PcFrame;

#endif
