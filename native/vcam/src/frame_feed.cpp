#include "frame_feed.h"

#include <new>

namespace {

constexpr wchar_t kPipeName[] = L"" PCVC_PIPE_NAME;

// How long to wait before trying the pipe again when the app is not there, and after a
// connection ends.
constexpr DWORD kRetryMs = 500;
constexpr DWORD kReconnectMs = 250;

// The final "stopped" status must not hold up Stop() if the app does not read.
constexpr DWORD kStatusTimeoutMs = 200;

// A byte-mode pipe never legitimately completes many empty reads in a row.
constexpr int kMaxEmptyTransfers = 64;

constexpr size_t kScratchSize = 64 * 1024;

} // namespace

FrameFeed& FrameFeed::Instance() {
    // Constructed on first use and destroyed when the DLL unloads, after every stream is gone.
    static FrameFeed feed;
    return feed;
}

void FrameFeed::AddConsumer() {
    ExclusiveGuard guard(control_lock_);
    if (++consumers_ != 1) {
        return;
    }

    try {
        latest_.assign(PCVC_NV12_SIZE, 0);
        back_.assign(PCVC_NV12_SIZE, 0);
        scratch_.assign(kScratchSize, 0);
    } catch (const std::bad_alloc&) {
        // Without buffers the camera keeps showing the placeholder; that beats failing Start.
        TraceLine(L"frame feed: out of memory");
        return;
    }

    stop_event_.reset(CreateEventW(nullptr, TRUE, FALSE, nullptr));
    if (!stop_event_.valid()) {
        return;
    }
    thread_.reset(CreateThread(nullptr, 0, ThreadMain, this, 0, nullptr));
    if (!thread_.valid()) {
        stop_event_.reset();
    }
}

void FrameFeed::RemoveConsumer() {
    ExclusiveGuard guard(control_lock_);
    if (consumers_ == 0 || --consumers_ != 0) {
        return;
    }

    if (thread_.valid()) {
        // The thread waits on the stop event in every blocking call, so this returns quickly.
        SetEvent(stop_event_.get());
        WaitForSingleObject(thread_.get(), INFINITE);
        thread_.reset();
    }
    stop_event_.reset();

    {
        ExclusiveGuard frame_guard(frame_lock_);
        has_frame_ = false;
        std::vector<uint8_t>().swap(latest_);
    }
    std::vector<uint8_t>().swap(back_);
    std::vector<uint8_t>().swap(scratch_);
}

DWORD WINAPI FrameFeed::ThreadMain(void* context) {
    static_cast<FrameFeed*>(context)->Run();
    return 0;
}

void FrameFeed::Run() {
    UniqueHandle io_event(CreateEventW(nullptr, TRUE, FALSE, nullptr));
    if (!io_event.valid()) {
        return;
    }
    const HANDLE stop = stop_event_.get();

    while (WaitForSingleObject(stop, 0) != WAIT_OBJECT_0) {
        // SECURITY_IDENTIFICATION: the pipe server may learn who we are but can never act as
        // the Frame Server's account. Any local user could create a pipe with this name first.
        UniqueHandle pipe(CreateFileW(kPipeName, GENERIC_READ | GENERIC_WRITE, 0, nullptr,
                                      OPEN_EXISTING,
                                      FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT |
                                          SECURITY_IDENTIFICATION,
                                      nullptr));
        if (!pipe.valid()) {
            // ERROR_FILE_NOT_FOUND: the app is not running. ERROR_PIPE_BUSY: it has not
            // created the next instance yet. Either way, try again shortly.
            WaitForSingleObject(stop, kRetryMs);
            continue;
        }

        TraceLine(L"frame feed: connected");
        Serve(pipe.get(), io_event.get());
        pipe.reset();
        MarkStale();
        TraceLine(L"frame feed: disconnected");
        WaitForSingleObject(stop, kReconnectMs);
    }
}

void FrameFeed::Serve(HANDLE pipe, HANDLE io_event) {
    const HANDLE stop = stop_event_.get();
    if (!SendStatus(pipe, io_event, true, stop)) {
        return;
    }

    Io result = Io::Done;
    while (result == Io::Done) {
        PcvcHeader header{};
        result = Transfer(pipe, io_event, false, &header, sizeof(header), stop, INFINITE);
        if (result != Io::Done) {
            break;
        }
        if (header.magic != PCVC_MAGIC || header.version != PCVC_VERSION ||
            header.payload_len > PCVC_MAX_PAYLOAD) {
            TraceLine(L"frame feed: bad header (magic %08X, version %u); reconnecting",
                      header.magic, header.version);
            result = Io::Failed;
            break;
        }

        uint32_t remaining = header.payload_len;
        if (header.type == PCVC_TYPE_FRAME && remaining >= sizeof(PcvcFrameInfo)) {
            PcvcFrameInfo info{};
            result = Transfer(pipe, io_event, false, &info, sizeof(info), stop, INFINITE);
            remaining -= sizeof(PcvcFrameInfo);
            if (result == Io::Done && info.width == PCVC_WIDTH && info.height == PCVC_HEIGHT &&
                remaining == PCVC_NV12_SIZE) {
                result = Transfer(pipe, io_event, false, back_.data(), PCVC_NV12_SIZE, stop,
                                  INFINITE);
                remaining = 0;
                if (result == Io::Done) {
                    ExclusiveGuard guard(frame_lock_);
                    latest_.swap(back_);
                    has_frame_ = true;
                    frame_tick_ = GetTickCount64();
                }
            }
        } else if (header.type == PCVC_TYPE_IDLE) {
            MarkStale();
        }

        // Skip whatever this version does not understand: unknown types, odd frame sizes.
        while (result == Io::Done && remaining > 0) {
            const DWORD chunk = static_cast<DWORD>(
                remaining < scratch_.size() ? remaining : scratch_.size());
            result = Transfer(pipe, io_event, false, scratch_.data(), chunk, stop, INFINITE);
            remaining -= chunk;
        }
    }

    if (result == Io::Stopped) {
        // Best effort: tell the app the camera is no longer in use before closing.
        SendStatus(pipe, io_event, false, nullptr);
    }
}

bool FrameFeed::SendStatus(HANDLE pipe, HANDLE io_event, bool streaming, HANDLE stop_event) {
    struct {
        PcvcHeader header;
        PcvcStatus status;
    } message{};
    static_assert(sizeof(message) == PCVC_HEADER_SIZE + PCVC_STATUS_SIZE, "status message");
    message.header.magic = PCVC_MAGIC;
    message.header.version = PCVC_VERSION;
    message.header.type = PCVC_TYPE_STATUS;
    message.header.payload_len = PCVC_STATUS_SIZE;
    message.status.streaming = streaming ? 1u : 0u;
    message.status.process_id = CurrentProcessId();
    return Transfer(pipe, io_event, true, &message, sizeof(message), stop_event,
                    kStatusTimeoutMs) == Io::Done;
}

FrameFeed::Io FrameFeed::Transfer(HANDLE pipe, HANDLE io_event, bool write, void* data,
                                  DWORD size, HANDLE stop_event, DWORD timeout_ms) {
    auto* bytes = static_cast<uint8_t*>(data);
    DWORD done = 0;
    int empty = 0;

    while (done < size) {
        OVERLAPPED overlapped{};
        overlapped.hEvent = io_event;
        ResetEvent(io_event);

        const BOOL started = write
            ? WriteFile(pipe, bytes + done, size - done, nullptr, &overlapped)
            : ReadFile(pipe, bytes + done, size - done, nullptr, &overlapped);
        if (!started && GetLastError() != ERROR_IO_PENDING) {
            return Io::Failed; // ERROR_BROKEN_PIPE and friends: the app went away.
        }

        if (!started) {
            const HANDLE handles[2] = {io_event, stop_event};
            const DWORD count = stop_event != nullptr ? 2 : 1;
            const DWORD wait = WaitForMultipleObjects(count, handles, FALSE, timeout_ms);
            if (wait != WAIT_OBJECT_0) {
                // Cancel, then wait for the cancellation to finish: the kernel owns
                // `overlapped` and the buffer until the I/O has completed.
                DWORD ignored = 0;
                CancelIoEx(pipe, &overlapped);
                GetOverlappedResult(pipe, &overlapped, &ignored, TRUE);
                return wait == WAIT_OBJECT_0 + 1 ? Io::Stopped : Io::Failed;
            }
        }

        DWORD transferred = 0;
        if (!GetOverlappedResult(pipe, &overlapped, &transferred, FALSE)) {
            return Io::Failed;
        }
        if (transferred == 0) {
            if (++empty > kMaxEmptyTransfers) {
                return Io::Failed;
            }
            continue;
        }
        empty = 0;
        done += transferred;
    }
    return Io::Done;
}

void FrameFeed::MarkStale() {
    ExclusiveGuard guard(frame_lock_);
    has_frame_ = false;
}
