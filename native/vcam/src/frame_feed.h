// The connection to the PairCam app: a named-pipe client that keeps the newest frame.
//
// One feed exists per process and is shared by every stream in it. It runs a background thread
// only while at least one stream is running (a "consumer"), so the app sees a connection exactly
// when some app is using the camera, and nothing runs inside the Frame Server otherwise.

#pragma once

#include "common.h"

class FrameFeed {
public:
    static FrameFeed& Instance();

    // A stream started or stopped. The first consumer starts the pipe thread; the last one
    // stops and joins it. Never called while holding a stream lock that the thread could need.
    void AddConsumer();
    void RemoveConsumer();

    // Calls `use` with the newest frame (tightly packed NV12, PCVC_WIDTH x PCVC_HEIGHT) while
    // holding a shared lock, if a frame arrived within PCVC_STALE_MS. Returns false otherwise,
    // and the caller shows the placeholder. `use` must be quick (one copy or conversion).
    template <class Use>
    bool WithNewest(Use&& use) {
        SharedGuard guard(frame_lock_);
        if (!has_frame_ || GetTickCount64() - frame_tick_ > PCVC_STALE_MS) {
            return false;
        }
        use(latest_.data());
        return true;
    }

private:
    FrameFeed() = default;

    enum class Io { Done, Stopped, Failed };

    static DWORD WINAPI ThreadMain(void* context);
    void Run();
    void Serve(HANDLE pipe, HANDLE io_event);
    Io Transfer(HANDLE pipe, HANDLE io_event, bool write, void* data, DWORD size,
                HANDLE stop_event, DWORD timeout_ms);
    bool SendStatus(HANDLE pipe, HANDLE io_event, bool streaming, HANDLE stop_event);
    void MarkStale();

    // Thread control, serialised by control_lock_.
    SrwLock control_lock_;
    long consumers_ = 0;
    UniqueHandle thread_;
    UniqueHandle stop_event_;

    // The newest frame. The pipe thread fills `back_` without a lock and swaps it in.
    SrwLock frame_lock_;
    std::vector<uint8_t> latest_;
    bool has_frame_ = false;
    ULONGLONG frame_tick_ = 0;

    // Touched by the pipe thread only.
    std::vector<uint8_t> back_;
    std::vector<uint8_t> scratch_;
};
