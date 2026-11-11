// PairCam virtual camera: the contract between the desktop app and the camera DLL.
//
// This header is the single specification of everything the two sides share: the COM class id,
// the pipe name and the binary framing on the pipe. It is plain C so the C++ DLL and the test
// harness include it directly; the Rust side (`crates/engine/src/vcam.rs`) mirrors the constants
// and a unit test there parses this file to make sure both stay identical. Change the version
// whenever the layout changes.
//
// Transport
// ---------
// The DLL runs inside the Windows Camera Frame Server service (session 0, Local Service), while
// the PairCam app runs as the signed-in user in their own session. Named pipes work across that
// boundary without privileges, so:
//
// - The APP is the pipe server. It creates `\\.\pipe\PairCam.Camera` in byte mode with
//   unlimited instances and a security descriptor that admits Local Service, Local System,
//   Administrators and the current user.
// - The DLL is the client. It connects only while some app is streaming from the camera, and
//   reconnects in the background (every 500 ms) when the app is not there.
//
// Messages
// --------
// All integers are little-endian. Every message starts with the same 16-byte header:
//
//   offset size field
//   0      4    magic        PCVC_MAGIC ("PCVC")
//   4      2    version      PCVC_VERSION
//   6      2    type         PCVC_TYPE_*
//   8      4    payload_len  bytes that follow the header
//   12     4    reserved     zero
//
// App -> DLL:
//   PCVC_TYPE_FRAME   payload = PcvcFrameInfo (16 bytes) followed by exactly
//                     width * height * 3 / 2 bytes of tightly packed NV12 (the Y plane with a
//                     stride of `width`, then the interleaved UV plane with a stride of `width`).
//                     The app always sends PCVC_WIDTH x PCVC_HEIGHT; the DLL never scales and
//                     drops frames of any other size. The DLL keeps only the newest frame.
//   PCVC_TYPE_IDLE    payload_len = 0. The phone stopped; show the placeholder right away
//                     instead of after PCVC_STALE_MS.
//
// DLL -> app:
//   PCVC_TYPE_STATUS  payload = PcvcStatus (8 bytes). Sent right after connecting
//                     (streaming = 1) and before a clean disconnect (streaming = 0). A dropped
//                     connection also means "not streaming".
//
// A reader that sees a bad magic, an unknown version or a payload longer than
// PCVC_MAX_PAYLOAD drops the connection; the DLL then reconnects. Unknown message types with a
// sane length are skipped.

#ifndef PAIRCAM_VCAM_H
#define PAIRCAM_VCAM_H

#include <stdint.h>

// The COM class of the media source. Registered in HKLM by `DllRegisterServer` and passed to
// MFCreateVirtualCamera by the app.
#define PCVC_CLSID_STRING "{81785F8A-A3B5-4F45-BC7B-F92794EB45BB}"
#define PCVC_CLSID_INIT                                                                           \
    {0x81785f8a, 0xa3b5, 0x4f45, {0xbc, 0x7b, 0xf9, 0x27, 0x94, 0xeb, 0x45, 0xbb}}

// The name apps show for the camera.
#define PCVC_FRIENDLY_NAME "PairCam"

#define PCVC_PIPE_NAME "\\\\.\\pipe\\PairCam.Camera"

#define PCVC_MAGIC 0x43564350u /* "PCVC" in memory order */
#define PCVC_VERSION 1u

#define PCVC_TYPE_FRAME 1u
#define PCVC_TYPE_IDLE 2u
#define PCVC_TYPE_STATUS 3u

// The one picture size the camera offers, and its frame rate.
#define PCVC_WIDTH 1920u
#define PCVC_HEIGHT 1080u
#define PCVC_FPS 30u
#define PCVC_NV12_SIZE (PCVC_WIDTH * PCVC_HEIGHT * 3u / 2u)

// After this long without a frame the camera shows the placeholder again.
#define PCVC_STALE_MS 1000u

// Upper bound on any payload, to reject garbage before allocating.
#define PCVC_MAX_PAYLOAD (64u * 1024u * 1024u)

#define PCVC_HEADER_SIZE 16u
#define PCVC_FRAME_INFO_SIZE 16u
#define PCVC_STATUS_SIZE 8u

#pragma pack(push, 1)

typedef struct PcvcHeader {
    uint32_t magic;
    uint16_t version;
    uint16_t type;
    uint32_t payload_len;
    uint32_t reserved;
} PcvcHeader;

typedef struct PcvcFrameInfo {
    uint32_t width;
    uint32_t height;
    // When the phone captured the picture, in microseconds on the app's clock. Informational:
    // the DLL stamps samples with its own steady clock.
    uint64_t timestamp_us;
} PcvcFrameInfo;

typedef struct PcvcStatus {
    // 1 while an app is streaming from the camera, 0 when it stops.
    uint32_t streaming;
    // The id of the process hosting the DLL (usually the Frame Server), for diagnostics.
    uint32_t process_id;
} PcvcStatus;

#pragma pack(pop)

#ifdef __cplusplus
static_assert(sizeof(PcvcHeader) == PCVC_HEADER_SIZE, "header layout");
static_assert(sizeof(PcvcFrameInfo) == PCVC_FRAME_INFO_SIZE, "frame info layout");
static_assert(sizeof(PcvcStatus) == PCVC_STATUS_SIZE, "status layout");
#endif

#endif // PAIRCAM_VCAM_H
