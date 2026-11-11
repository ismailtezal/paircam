// The picture the camera shows when no phone video is arriving.

#pragma once

#include <cstdint>
#include <vector>

struct PlaceholderPicture {
    // PCVC_WIDTH x PCVC_HEIGHT, tightly packed.
    std::vector<uint8_t> nv12;
    std::vector<uint8_t> bgra;
    // False when text rendering failed and the picture is the plain dark frame.
    bool has_text = false;
};

// Renders the placeholder once per process (thread-safe) and returns it. Rendering uses
// Direct2D and DirectWrite in software on a short-lived thread of its own, so it does not depend
// on the caller's COM apartment. Any failure falls back to a plain dark frame; this never fails.
const PlaceholderPicture& Placeholder();
