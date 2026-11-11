// Shared includes and small helpers for the PairCam camera DLL.
//
// The DLL is plain COM written by hand (no WIL, no C++/WinRT, no ATL) so it builds with nothing
// but MSVC and the Windows SDK, links the C runtime statically, and keeps every lifetime rule
// visible in the code. Structure and Frame Server requirements follow smourier/VCamSample (MIT,
// see the notice in media_source.cpp).

#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif

#include <windows.h>

#include <initguid.h>
// IKsControl comes from ksproxy.h, which needs the COM interface macros from unknwn.h first.
#include <unknwn.h>

#include <ks.h>
#include <ksproxy.h>
#include <ksmedia.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mfvirtualcamera.h>
#include <propvarutil.h>
#include <wrl/client.h>

#include <atomic>
#include <cstdint>
#include <cstring>
#include <new>
#include <vector>

#include "paircam_vcam.h"

using Microsoft::WRL::ComPtr;

// The CLSID of the media source, from the shared header.
extern const CLSID CLSID_PairCamVirtualCamera;

// Live COM objects and server locks, for DllCanUnloadNow.
extern std::atomic<long> g_objectCount;
extern std::atomic<long> g_lockCount;

// Counts one live COM object for as long as it exists.
struct ModuleObject {
    ModuleObject() { g_objectCount.fetch_add(1); }
    ~ModuleObject() { g_objectCount.fetch_sub(1); }
    ModuleObject(const ModuleObject&) = delete;
    ModuleObject& operator=(const ModuleObject&) = delete;
};

#define PC_RETURN_IF_FAILED(expression)                                                          \
    do {                                                                                          \
        HRESULT pc_hr_ = (expression);                                                            \
        if (FAILED(pc_hr_)) {                                                                     \
            return pc_hr_;                                                                        \
        }                                                                                         \
    } while (0)

// A slim reader/writer lock. SRW locks need no cleanup and never fail, which suits code that
// runs inside a shared service process.
class SrwLock {
public:
    SrwLock() = default;
    SrwLock(const SrwLock&) = delete;
    SrwLock& operator=(const SrwLock&) = delete;

    void lock() { AcquireSRWLockExclusive(&lock_); }
    void unlock() { ReleaseSRWLockExclusive(&lock_); }
    void lock_shared() { AcquireSRWLockShared(&lock_); }
    void unlock_shared() { ReleaseSRWLockShared(&lock_); }

private:
    SRWLOCK lock_ = SRWLOCK_INIT;
};

class ExclusiveGuard {
public:
    explicit ExclusiveGuard(SrwLock& lock) : lock_(lock) { lock_.lock(); }
    ~ExclusiveGuard() { lock_.unlock(); }
    ExclusiveGuard(const ExclusiveGuard&) = delete;
    ExclusiveGuard& operator=(const ExclusiveGuard&) = delete;

private:
    SrwLock& lock_;
};

class SharedGuard {
public:
    explicit SharedGuard(SrwLock& lock) : lock_(lock) { lock_.lock_shared(); }
    ~SharedGuard() { lock_.unlock_shared(); }
    SharedGuard(const SharedGuard&) = delete;
    SharedGuard& operator=(const SharedGuard&) = delete;

private:
    SrwLock& lock_;
};

// Owns a kernel handle (event, thread, timer, pipe). Closes it exactly once.
class UniqueHandle {
public:
    UniqueHandle() = default;
    explicit UniqueHandle(HANDLE handle) : handle_(handle) {}
    ~UniqueHandle() { reset(); }
    UniqueHandle(const UniqueHandle&) = delete;
    UniqueHandle& operator=(const UniqueHandle&) = delete;

    HANDLE get() const { return handle_; }
    bool valid() const { return handle_ != nullptr && handle_ != INVALID_HANDLE_VALUE; }

    void reset(HANDLE handle = nullptr) {
        if (valid()) {
            CloseHandle(handle_);
        }
        handle_ = handle;
    }

private:
    HANDLE handle_ = nullptr;
};

// Writes a line to the debugger output (visible with DebugView or a kernel debugger attached to
// the Frame Server). Cheap when no debugger listens.
void TraceLine(const wchar_t* format, ...);

// The id of the process that loaded the DLL.
inline uint32_t CurrentProcessId() { return static_cast<uint32_t>(GetCurrentProcessId()); }
