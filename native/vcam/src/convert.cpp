#include "convert.h"

#include <cstring>
#include <initializer_list>

namespace {

inline uint8_t Clamp255(int value) {
    return static_cast<uint8_t>(value < 0 ? 0 : (value > 255 ? 255 : value));
}

// BT.709 limited range to full range RGB, in 16.16 fixed point.
constexpr int kLuma = 76309;      // 255 / 219
constexpr int kRedV = 117489;     // 1.5748 * 255 / 224
constexpr int kGreenU = 13975;    // 0.1873 * 255 / 224
constexpr int kGreenV = 34925;    // 0.4681 * 255 / 224
constexpr int kBlueU = 138438;    // 1.8556 * 255 / 224
constexpr int kRound = 1 << 15;

inline void StorePixel(uint8_t* out, int luma, int red, int green, int blue) {
    out[0] = Clamp255((luma + blue + kRound) >> 16);
    out[1] = Clamp255((luma + green + kRound) >> 16);
    out[2] = Clamp255((luma + red + kRound) >> 16);
    out[3] = 0xFF;
}

} // namespace

void CopyNv12(const uint8_t* packed, uint32_t width, uint32_t height, uint8_t* y, long y_pitch,
              uint8_t* uv, long uv_pitch) {
    const uint8_t* source_y = packed;
    const uint8_t* source_uv = packed + static_cast<size_t>(width) * height;
    if (y_pitch == static_cast<long>(width) && uv == y + static_cast<size_t>(width) * height &&
        uv_pitch == static_cast<long>(width)) {
        std::memcpy(y, packed, static_cast<size_t>(width) * height * 3 / 2);
        return;
    }
    for (uint32_t row = 0; row < height; ++row) {
        std::memcpy(y + static_cast<ptrdiff_t>(row) * y_pitch, source_y + row * width, width);
    }
    for (uint32_t row = 0; row < height / 2; ++row) {
        std::memcpy(uv + static_cast<ptrdiff_t>(row) * uv_pitch, source_uv + row * width, width);
    }
}

void Nv12ToRgb32(const uint8_t* packed, uint32_t width, uint32_t height, uint8_t* rgb,
                 long rgb_pitch) {
    const uint8_t* plane_y = packed;
    const uint8_t* plane_uv = packed + static_cast<size_t>(width) * height;

    // Two output rows share one chroma row; each chroma pair covers two columns.
    for (uint32_t row = 0; row < height; row += 2) {
        const uint8_t* y0 = plane_y + static_cast<size_t>(row) * width;
        const uint8_t* y1 = y0 + width;
        const uint8_t* chroma = plane_uv + static_cast<size_t>(row / 2) * width;
        uint8_t* out0 = rgb + static_cast<ptrdiff_t>(row) * rgb_pitch;
        uint8_t* out1 = out0 + rgb_pitch;
        const bool has_second_row = row + 1 < height;

        for (uint32_t column = 0; column < width; column += 2) {
            const int u = chroma[column] - 128;
            const int v = chroma[column + 1] - 128;
            const int red = kRedV * v;
            const int green = -kGreenU * u - kGreenV * v;
            const int blue = kBlueU * u;

            StorePixel(out0 + column * 4, kLuma * (y0[column] - 16), red, green, blue);
            StorePixel(out0 + column * 4 + 4, kLuma * (y0[column + 1] - 16), red, green, blue);
            if (has_second_row) {
                StorePixel(out1 + column * 4, kLuma * (y1[column] - 16), red, green, blue);
                StorePixel(out1 + column * 4 + 4, kLuma * (y1[column + 1] - 16), red, green,
                           blue);
            }
        }
    }
}

void Bgra32ToNv12(const uint8_t* bgra, uint32_t width, uint32_t height, uint8_t* packed) {
    uint8_t* plane_y = packed;
    uint8_t* plane_uv = packed + static_cast<size_t>(width) * height;

    // BT.709 limited range, 8.8 fixed point (coefficients already scaled by 219/255, 224/255).
    for (uint32_t row = 0; row < height; ++row) {
        const uint8_t* in = bgra + static_cast<size_t>(row) * width * 4;
        uint8_t* out = plane_y + static_cast<size_t>(row) * width;
        for (uint32_t column = 0; column < width; ++column) {
            const int b = in[column * 4 + 0];
            const int g = in[column * 4 + 1];
            const int r = in[column * 4 + 2];
            out[column] = Clamp255(((47 * r + 157 * g + 16 * b + 128) >> 8) + 16);
        }
    }
    for (uint32_t row = 0; row < height; row += 2) {
        const uint8_t* top = bgra + static_cast<size_t>(row) * width * 4;
        const uint8_t* bottom = row + 1 < height ? top + static_cast<size_t>(width) * 4 : top;
        uint8_t* out = plane_uv + static_cast<size_t>(row / 2) * width;
        for (uint32_t column = 0; column < width; column += 2) {
            int b = 0;
            int g = 0;
            int r = 0;
            for (const uint8_t* line : {top, bottom}) {
                b += line[column * 4 + 0] + line[column * 4 + 4];
                g += line[column * 4 + 1] + line[column * 4 + 5];
                r += line[column * 4 + 2] + line[column * 4 + 6];
            }
            b = (b + 2) / 4;
            g = (g + 2) / 4;
            r = (r + 2) / 4;
            // The chroma rows sum to zero, so greys land exactly on 128.
            out[column] = Clamp255(((-26 * r - 86 * g + 112 * b + 128) >> 8) + 128);
            out[column + 1] = Clamp255(((112 * r - 102 * g - 10 * b + 128) >> 8) + 128);
        }
    }
}

void CopyRgb32(const uint8_t* packed, uint32_t width, uint32_t height, uint8_t* rgb,
               long rgb_pitch) {
    const size_t row_bytes = static_cast<size_t>(width) * 4;
    if (rgb_pitch == static_cast<long>(row_bytes)) {
        std::memcpy(rgb, packed, row_bytes * height);
        return;
    }
    for (uint32_t row = 0; row < height; ++row) {
        std::memcpy(rgb + static_cast<ptrdiff_t>(row) * rgb_pitch, packed + row * row_bytes,
                    row_bytes);
    }
}
