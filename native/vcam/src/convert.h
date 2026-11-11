// Pixel conversions on the CPU.
//
// The camera's NV12 is BT.709 with limited (video) range, which is what H.264 and HEVC streams
// from a phone carry at HD sizes; both directions here use that matrix so a round trip stays
// stable. RGB32 means Media Foundation's X8R8G8B8: bytes B, G, R, X in memory.

#pragma once

#include <cstdint>

// Copies a tightly packed NV12 picture (`width * height * 3 / 2` bytes) into planes with any
// stride. `uv` may point anywhere, not only right after the Y plane.
void CopyNv12(const uint8_t* packed, uint32_t width, uint32_t height, uint8_t* y, long y_pitch,
              uint8_t* uv, long uv_pitch);

// Converts a tightly packed NV12 picture to RGB32. `rgb_pitch` may be negative (bottom-up).
void Nv12ToRgb32(const uint8_t* packed, uint32_t width, uint32_t height, uint8_t* rgb,
                 long rgb_pitch);

// Converts tightly packed BGRA to tightly packed NV12 (used once, for the placeholder).
void Bgra32ToNv12(const uint8_t* bgra, uint32_t width, uint32_t height, uint8_t* packed);

// Copies a tightly packed 32-bit picture into rows with any pitch.
void CopyRgb32(const uint8_t* packed, uint32_t width, uint32_t height, uint8_t* rgb,
               long rgb_pitch);
