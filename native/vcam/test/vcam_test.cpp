// Test harness for paircam_vcam.dll. Needs no administrator rights and no registration.
//
// It loads the DLL directly, creates the camera through DllGetClassObject the way Frame Server
// would after reading the registry, and drives the media source like a capture pipeline:
// select a media type, start, request samples, read them back. A pipe server inside the harness
// plays the PairCam app.
//
// Usage: paircam_vcam_test.exe [--external-app] [path\to\paircam_vcam.dll]
//   --external-app  skip the built-in tests and check frames from a PairCam pipe server that is
//                   already running (used by the Rust end-to-end test).
// Exit code: the number of failed checks.

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif

#include <windows.h>

#include <initguid.h>
#include <unknwn.h>

#include <ks.h>
#include <ksproxy.h>
#include <ksmedia.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mfvirtualcamera.h>
#include <timeapi.h>
#include <tlhelp32.h>
#include <wrl/client.h>

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

#include "paircam_vcam.h"

using Microsoft::WRL::ComPtr;

namespace {

const CLSID kClsid = PCVC_CLSID_INIT;
constexpr wchar_t kPipeName[] = L"" PCVC_PIPE_NAME;
constexpr LONGLONG kFrameDuration = 10'000'000 / PCVC_FPS;
constexpr size_t kRgbSize = static_cast<size_t>(PCVC_WIDTH) * PCVC_HEIGHT * 4;

int g_failures = 0;
int g_passes = 0;

double NowMs();

void Check(bool condition, const char* name, const char* format = "", ...) {
    char detail[512] = {};
    va_list arguments;
    va_start(arguments, format);
    vsnprintf(detail, sizeof(detail), format, arguments);
    va_end(arguments);
    static const double start = NowMs();
    std::printf("%s  [%6.2f s] %s%s%s\n", condition ? "PASS" : "FAIL", (NowMs() - start) / 1000.0,
                name, detail[0] ? ": " : "", detail);
    std::fflush(stdout);
    (condition ? g_passes : g_failures)++;
}

double NowMs() {
    static LARGE_INTEGER frequency = [] {
        LARGE_INTEGER value;
        QueryPerformanceFrequency(&value);
        return value;
    }();
    LARGE_INTEGER counter;
    QueryPerformanceCounter(&counter);
    return static_cast<double>(counter.QuadPart) * 1000.0 / static_cast<double>(frequency.QuadPart);
}

DWORD ThreadCount() {
    DWORD count = 0;
    HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
    if (snapshot == INVALID_HANDLE_VALUE) {
        return 0;
    }
    THREADENTRY32 entry{sizeof(entry)};
    if (Thread32First(snapshot, &entry)) {
        do {
            if (entry.th32OwnerProcessID == GetCurrentProcessId()) {
                ++count;
            }
        } while (Thread32Next(snapshot, &entry));
    }
    CloseHandle(snapshot);
    return count;
}

DWORD HandleCount() {
    DWORD count = 0;
    GetProcessHandleCount(GetCurrentProcess(), &count);
    return count;
}

// ----------------------------------------------------------------------------------------------
// Test pictures
// ----------------------------------------------------------------------------------------------

std::vector<uint8_t> MakePattern(uint32_t seed) {
    std::vector<uint8_t> nv12(PCVC_NV12_SIZE);
    uint8_t* y = nv12.data();
    uint8_t* uv = y + PCVC_WIDTH * PCVC_HEIGHT;
    for (uint32_t row = 0; row < PCVC_HEIGHT; ++row) {
        for (uint32_t column = 0; column < PCVC_WIDTH; ++column) {
            y[row * PCVC_WIDTH + column] = static_cast<uint8_t>(column * 3 + row * 7 + seed);
        }
    }
    for (uint32_t row = 0; row < PCVC_HEIGHT / 2; ++row) {
        for (uint32_t column = 0; column < PCVC_WIDTH; column += 2) {
            uv[row * PCVC_WIDTH + column] = static_cast<uint8_t>(column * 5 + row * 3 + seed);
            uv[row * PCVC_WIDTH + column + 1] =
                static_cast<uint8_t>(column + row * 11 + seed * 3);
        }
    }
    return nv12;
}

// The BT.709 limited-range conversion, computed independently in floating point.
void ExpectedRgb(const std::vector<uint8_t>& nv12, uint32_t x, uint32_t y, int out[3]) {
    const double luma = 1.164383 * (nv12[y * PCVC_WIDTH + x] - 16.0);
    const size_t chroma = PCVC_WIDTH * PCVC_HEIGHT + (y / 2) * PCVC_WIDTH + (x & ~1u);
    const double u = nv12[chroma] - 128.0;
    const double v = nv12[chroma + 1] - 128.0;
    const double rgb[3] = {luma + 1.792741 * v, luma - 0.213249 * u - 0.532909 * v,
                           luma + 2.112402 * u};
    for (int i = 0; i < 3; ++i) {
        out[i] = static_cast<int>(std::lround(std::clamp(rgb[i], 0.0, 255.0)));
    }
}

// ----------------------------------------------------------------------------------------------
// The app side: a pipe server like the one in crates/engine/src/vcam.rs
// ----------------------------------------------------------------------------------------------

class PipeServer {
public:
    ~PipeServer() { Close(); }

    bool Listen() {
        Close();
        pipe_ = CreateNamedPipeW(kPipeName, PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                                 PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                                 PIPE_UNLIMITED_INSTANCES, 1 << 20, 4096, 0, nullptr);
        return pipe_ != INVALID_HANDLE_VALUE;
    }

    bool Accept(DWORD timeout_ms) {
        OVERLAPPED overlapped{};
        overlapped.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        bool connected = false;
        if (ConnectNamedPipe(pipe_, &overlapped)) {
            connected = true;
        } else if (GetLastError() == ERROR_PIPE_CONNECTED) {
            connected = true;
        } else if (GetLastError() == ERROR_IO_PENDING) {
            DWORD ignored = 0;
            if (WaitForSingleObject(overlapped.hEvent, timeout_ms) == WAIT_OBJECT_0) {
                connected = GetOverlappedResult(pipe_, &overlapped, &ignored, FALSE) != 0;
            } else {
                CancelIoEx(pipe_, &overlapped);
                GetOverlappedResult(pipe_, &overlapped, &ignored, TRUE);
            }
        }
        CloseHandle(overlapped.hEvent);
        return connected;
    }

    bool Transfer(bool write, void* data, DWORD size, DWORD timeout_ms) {
        auto* bytes = static_cast<uint8_t*>(data);
        DWORD done = 0;
        do {
            OVERLAPPED overlapped{};
            overlapped.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
            const BOOL started = write
                ? WriteFile(pipe_, bytes + done, size - done, nullptr, &overlapped)
                : ReadFile(pipe_, bytes + done, size - done, nullptr, &overlapped);
            DWORD transferred = 0;
            bool ok = false;
            if (started || GetLastError() == ERROR_IO_PENDING) {
                if (WaitForSingleObject(overlapped.hEvent, timeout_ms) == WAIT_OBJECT_0) {
                    ok = GetOverlappedResult(pipe_, &overlapped, &transferred, FALSE) != 0;
                } else {
                    CancelIoEx(pipe_, &overlapped);
                    GetOverlappedResult(pipe_, &overlapped, &transferred, TRUE);
                }
            }
            CloseHandle(overlapped.hEvent);
            if (!ok) {
                return false;
            }
            done += transferred;
        } while (done < size);
        return true;
    }

    // Reads one status message. Returns -1 on timeout or disconnect.
    int ReadStatus(DWORD timeout_ms) {
        struct {
            PcvcHeader header;
            PcvcStatus status;
        } message{};
        if (!Transfer(false, &message, sizeof(message), timeout_ms)) {
            return -1;
        }
        if (message.header.magic != PCVC_MAGIC || message.header.version != PCVC_VERSION ||
            message.header.type != PCVC_TYPE_STATUS ||
            message.header.payload_len != PCVC_STATUS_SIZE) {
            return -2;
        }
        return static_cast<int>(message.status.streaming);
    }

    bool SendFrame(const std::vector<uint8_t>& nv12, uint32_t width = PCVC_WIDTH,
                   uint32_t height = PCVC_HEIGHT) {
        PcvcHeader header{PCVC_MAGIC, PCVC_VERSION, PCVC_TYPE_FRAME,
                          static_cast<uint32_t>(sizeof(PcvcFrameInfo) + nv12.size()), 0};
        PcvcFrameInfo info{width, height, 0};
        return Transfer(true, &header, sizeof(header), 2000) &&
               Transfer(true, &info, sizeof(info), 2000) &&
               Transfer(true, const_cast<uint8_t*>(nv12.data()),
                        static_cast<DWORD>(nv12.size()), 2000);
    }

    bool SendRaw(const void* data, DWORD size) {
        return Transfer(true, const_cast<void*>(data), size, 2000);
    }

    bool SendHeader(uint16_t type, uint32_t payload_len, uint32_t magic = PCVC_MAGIC) {
        PcvcHeader header{magic, PCVC_VERSION, type, payload_len, 0};
        return SendRaw(&header, sizeof(header));
    }

    void Close() {
        if (pipe_ != INVALID_HANDLE_VALUE) {
            DisconnectNamedPipe(pipe_);
            CloseHandle(pipe_);
            pipe_ = INVALID_HANDLE_VALUE;
        }
    }

private:
    HANDLE pipe_ = INVALID_HANDLE_VALUE;
};

// Sends the same frame every 20 ms on its own thread, like the app's writer thread.
class FramePump {
public:
    FramePump(PipeServer& server, const std::vector<uint8_t>& frame)
        : server_(server), frame_(frame), thread_([this] { Run(); }) {}
    ~FramePump() { Stop(); }

    void Stop() {
        running_ = false;
        if (thread_.joinable()) {
            thread_.join();
        }
    }

    int sent() const { return sent_; }

private:
    void Run() {
        while (running_) {
            if (!server_.SendFrame(frame_)) {
                break;
            }
            ++sent_;
            Sleep(20);
        }
    }

    PipeServer& server_;
    std::vector<uint8_t> frame_;
    std::atomic<bool> running_{true};
    std::atomic<int> sent_{0};
    std::thread thread_;
};

// ----------------------------------------------------------------------------------------------
// The consumer side: what Frame Server does with the source
// ----------------------------------------------------------------------------------------------

using GetClassObjectFunction = HRESULT(STDAPICALLTYPE*)(REFCLSID, REFIID, void**);
using CanUnloadNowFunction = HRESULT(STDAPICALLTYPE*)();

GetClassObjectFunction g_get_class_object = nullptr;
CanUnloadNowFunction g_can_unload_now = nullptr;

struct Sample {
    LONGLONG time = 0;
    LONGLONG duration = 0;
    double arrived_ms = 0;
    bool has_token = false;
    std::vector<uint8_t> data;
};

class Camera {
public:
    HRESULT Activate() {
        ComPtr<IClassFactory> factory;
        HRESULT hr = g_get_class_object(kClsid, IID_PPV_ARGS(&factory));
        if (FAILED(hr)) {
            return hr;
        }
        hr = factory->CreateInstance(nullptr, IID_PPV_ARGS(&activate_));
        if (FAILED(hr)) {
            return hr;
        }
        return activate_->ActivateObject(IID_PPV_ARGS(&source_));
    }

    IMFActivate* activate() { return activate_.Get(); }
    IMFMediaSourceEx* source() { return source_.Get(); }
    IMFMediaStream* stream() { return stream_.Get(); }

    // Selects the media type at `type_index` (0 = NV12, 1 = RGB32) and starts the source.
    HRESULT Start(DWORD type_index, bool provide_allocator) {
        if (provide_allocator) {
            // What Frame Server does: hand the stream a sample allocator before starting.
            ComPtr<IMFSampleAllocatorControl> control;
            HRESULT hr = source_.As(&control);
            if (FAILED(hr)) {
                return hr;
            }
            DWORD input = 99;
            MFSampleAllocatorUsage usage{};
            hr = control->GetAllocatorUsage(0, &input, &usage);
            if (FAILED(hr) || usage != MFSampleAllocatorUsage_UsesProvidedAllocator) {
                return E_FAIL;
            }
            ComPtr<IMFVideoSampleAllocatorEx> allocator;
            hr = MFCreateVideoSampleAllocatorEx(IID_PPV_ARGS(&allocator));
            if (FAILED(hr)) {
                return hr;
            }
            hr = control->SetDefaultAllocator(0, allocator.Get());
            if (FAILED(hr)) {
                return hr;
            }
        }

        ComPtr<IMFPresentationDescriptor> presentation;
        HRESULT hr = source_->CreatePresentationDescriptor(&presentation);
        if (FAILED(hr)) {
            return hr;
        }
        BOOL selected = FALSE;
        ComPtr<IMFStreamDescriptor> descriptor;
        ComPtr<IMFMediaTypeHandler> handler;
        ComPtr<IMFMediaType> type;
        if (FAILED(hr = presentation->GetStreamDescriptorByIndex(0, &selected, &descriptor)) ||
            FAILED(hr = descriptor->GetMediaTypeHandler(&handler)) ||
            FAILED(hr = handler->GetMediaTypeByIndex(type_index, &type)) ||
            FAILED(hr = handler->SetCurrentMediaType(type.Get())) ||
            FAILED(hr = presentation->SelectStream(0))) {
            return hr;
        }

        PROPVARIANT start;
        PropVariantInit(&start);
        hr = source_->Start(presentation.Get(), &GUID_NULL, &start);
        if (FAILED(hr)) {
            return hr;
        }

        // The source announces the stream, then that it started.
        bool source_started = false;
        const double deadline = NowMs() + 3000;
        while (NowMs() < deadline && (!source_started || !stream_)) {
            ComPtr<IMFMediaEvent> event;
            if (FAILED(source_->GetEvent(MF_EVENT_FLAG_NO_WAIT, &event))) {
                Sleep(1);
                continue;
            }
            MediaEventType kind = MEUnknown;
            event->GetType(&kind);
            if (kind == MENewStream || kind == MEUpdatedStream) {
                PROPVARIANT value;
                PropVariantInit(&value);
                event->GetValue(&value);
                if (value.vt == VT_UNKNOWN && value.punkVal != nullptr) {
                    value.punkVal->QueryInterface(IID_PPV_ARGS(&stream_));
                }
                PropVariantClear(&value);
            } else if (kind == MESourceStarted) {
                source_started = true;
            }
        }
        if (!stream_ || !source_started) {
            return MF_E_INVALIDREQUEST;
        }
        return WaitForStreamEvent(MEStreamStarted, 3000) ? S_OK : MF_E_INVALIDREQUEST;
    }

    bool WaitForStreamEvent(MediaEventType wanted, DWORD timeout_ms) {
        const double deadline = NowMs() + timeout_ms;
        while (NowMs() < deadline) {
            ComPtr<IMFMediaEvent> event;
            if (FAILED(stream_->GetEvent(MF_EVENT_FLAG_NO_WAIT, &event))) {
                Sleep(1);
                continue;
            }
            MediaEventType kind = MEUnknown;
            event->GetType(&kind);
            if (kind == wanted) {
                return true;
            }
        }
        return false;
    }

    // Requests and collects `count` samples, keeping `outstanding` requests open like Frame
    // Server does. Returns false on timeout.
    bool Pull(size_t count, std::vector<Sample>& samples, int outstanding = 2,
              DWORD timeout_ms = 15000) {
        samples.clear();
        // Exactly `count` requests in total, at most `outstanding` open at a time, so nothing
        // is left over for the next call.
        size_t requested = 0;
        while (requested < count && requested < static_cast<size_t>(outstanding)) {
            if (FAILED(stream_->RequestSample(token_.Get()))) {
                return false;
            }
            ++requested;
        }
        const double deadline = NowMs() + timeout_ms;
        while (samples.size() < count && NowMs() < deadline) {
            ComPtr<IMFMediaEvent> event;
            if (FAILED(stream_->GetEvent(MF_EVENT_FLAG_NO_WAIT, &event))) {
                Sleep(1);
                continue;
            }
            MediaEventType kind = MEUnknown;
            event->GetType(&kind);
            if (kind != MEMediaSample) {
                continue;
            }
            const double arrived = NowMs();
            PROPVARIANT value;
            PropVariantInit(&value);
            event->GetValue(&value);
            ComPtr<IMFSample> sample;
            if (value.vt == VT_UNKNOWN && value.punkVal != nullptr) {
                value.punkVal->QueryInterface(IID_PPV_ARGS(&sample));
            }
            PropVariantClear(&value);
            if (!sample) {
                continue;
            }

            Sample record;
            record.arrived_ms = arrived;
            sample->GetSampleTime(&record.time);
            sample->GetSampleDuration(&record.duration);
            ComPtr<IUnknown> token;
            record.has_token = SUCCEEDED(sample->GetUnknown(MFSampleExtension_Token,
                                                            IID_PPV_ARGS(&token)));
            ComPtr<IMFMediaBuffer> buffer;
            if (SUCCEEDED(sample->ConvertToContiguousBuffer(&buffer))) {
                BYTE* data = nullptr;
                DWORD length = 0;
                if (SUCCEEDED(buffer->Lock(&data, nullptr, &length))) {
                    record.data.assign(data, data + length);
                    buffer->Unlock();
                }
            }
            samples.push_back(std::move(record));
            if (requested < count && SUCCEEDED(stream_->RequestSample(token_.Get()))) {
                ++requested;
            }
        }
        return samples.size() == count;
    }

    // Pulls until a sample satisfies `predicate` or `timeout_ms` passes.
    template <class Predicate>
    bool PullUntil(Predicate predicate, DWORD timeout_ms, Sample* found = nullptr) {
        const double deadline = NowMs() + timeout_ms;
        std::vector<Sample> samples;
        while (NowMs() < deadline) {
            if (!Pull(1, samples, 1, 2000)) {
                return false;
            }
            if (predicate(samples[0])) {
                if (found != nullptr) {
                    *found = std::move(samples[0]);
                }
                return true;
            }
        }
        return false;
    }

    HRESULT Stop() {
        const HRESULT hr = source_->Stop();
        if (SUCCEEDED(hr) && stream_) {
            WaitForStreamEvent(MEStreamStopped, 2000);
        }
        return hr;
    }

    void Shutdown() {
        if (source_) {
            source_->Shutdown();
        }
        if (activate_) {
            activate_->ShutdownObject();
            activate_->DetachObject();
        }
        stream_.Reset();
        source_.Reset();
        activate_.Reset();
    }

    Camera() { MFCreateAttributes(reinterpret_cast<IMFAttributes**>(token_.GetAddressOf()), 1); }

private:
    ComPtr<IMFActivate> activate_;
    ComPtr<IMFMediaSourceEx> source_;
    ComPtr<IMFMediaStream> stream_;
    ComPtr<IUnknown> token_;
};

// ----------------------------------------------------------------------------------------------
// Picture checks
// ----------------------------------------------------------------------------------------------

bool IsPlaceholderNv12(const std::vector<uint8_t>& data) {
    if (data.size() != PCVC_NV12_SIZE) {
        return false;
    }
    // Corners are the calm dark background (#0A0A0A -> Y about 25, neutral chroma).
    const uint8_t* uv = data.data() + PCVC_WIDTH * PCVC_HEIGHT;
    return data[0] >= 20 && data[0] <= 30 && data[PCVC_WIDTH * PCVC_HEIGHT - 1] >= 20 &&
           data[PCVC_WIDTH * PCVC_HEIGHT - 1] <= 30 && uv[0] >= 127 && uv[0] <= 129 &&
           uv[1] >= 127 && uv[1] <= 129;
}

size_t BrightPixelsInCentre(const std::vector<uint8_t>& nv12) {
    size_t bright = 0;
    for (uint32_t row = PCVC_HEIGHT / 3; row < PCVC_HEIGHT * 2 / 3; ++row) {
        for (uint32_t column = 0; column < PCVC_WIDTH; ++column) {
            bright += nv12[row * PCVC_WIDTH + column] > 120 ? 1 : 0;
        }
    }
    return bright;
}

// Writes a top-down RGB32 picture as a BMP next to the harness. Returns the path, or empty.
std::wstring SaveBitmap(const std::vector<uint8_t>& rgb, const wchar_t* name) {
    wchar_t self[MAX_PATH];
    GetModuleFileNameW(nullptr, self, MAX_PATH);
    std::wstring path = self;
    path = path.substr(0, path.find_last_of(L'\\') + 1) + name;

    BITMAPINFOHEADER info{};
    info.biSize = sizeof(info);
    info.biWidth = PCVC_WIDTH;
    info.biHeight = -static_cast<LONG>(PCVC_HEIGHT); // top-down
    info.biPlanes = 1;
    info.biBitCount = 32;
    info.biCompression = BI_RGB;
    BITMAPFILEHEADER file{};
    file.bfType = 0x4D42;
    file.bfOffBits = sizeof(file) + sizeof(info);
    file.bfSize = file.bfOffBits + static_cast<DWORD>(rgb.size());

    FILE* out = nullptr;
    if (_wfopen_s(&out, path.c_str(), L"wb") != 0 || out == nullptr) {
        return {};
    }
    fwrite(&file, sizeof(file), 1, out);
    fwrite(&info, sizeof(info), 1, out);
    fwrite(rgb.data(), 1, rgb.size(), out);
    fclose(out);
    return path;
}

bool IsPlaceholderRgb(const std::vector<uint8_t>& data) {
    return data.size() == kRgbSize && data[0] >= 8 && data[0] <= 12 && data[1] >= 8 &&
           data[1] <= 12 && data[2] >= 8 && data[2] <= 12;
}

// ----------------------------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------------------------

void TestActivationAndMediaTypes() {
    std::printf("\n== Activation and media types\n");
    Camera camera;
    HRESULT hr = camera.Activate();
    Check(SUCCEEDED(hr), "DllGetClassObject -> IClassFactory -> IMFActivate -> IMFMediaSourceEx",
          "hr=0x%08X", hr);
    if (FAILED(hr)) {
        return;
    }

    GUID clsid = GUID_NULL;
    camera.activate()->GetGUID(MFT_TRANSFORM_CLSID_Attribute, &clsid);
    UINT32 provides = 0;
    camera.activate()->GetUINT32(MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES, &provides);
    Check(clsid == kClsid && provides == 1, "activator attributes",
          "MFT_TRANSFORM_CLSID_Attribute matches, PROVIDE_ASSOCIATED_CAMERA_SOURCES=%u", provides);

    DWORD characteristics = 0;
    camera.source()->GetCharacteristics(&characteristics);
    Check(characteristics == MFMEDIASOURCE_IS_LIVE, "source is live", "0x%X", characteristics);

    ComPtr<IMFAttributes> source_attributes;
    ComPtr<IUnknown> profiles;
    hr = camera.source()->GetSourceAttributes(&source_attributes);
    if (SUCCEEDED(hr)) {
        hr = source_attributes->GetUnknown(MF_DEVICEMFT_SENSORPROFILE_COLLECTION,
                                           IID_PPV_ARGS(&profiles));
    }
    Check(SUCCEEDED(hr), "source attributes carry the sensor profile collection");

    ComPtr<IMFAttributes> stream_attributes;
    GUID category = GUID_NULL;
    UINT32 shared = 0;
    UINT32 types = 0;
    hr = camera.source()->GetStreamAttributes(0, &stream_attributes);
    if (SUCCEEDED(hr)) {
        stream_attributes->GetGUID(MF_DEVICESTREAM_STREAM_CATEGORY, &category);
        stream_attributes->GetUINT32(MF_DEVICESTREAM_FRAMESERVER_SHARED, &shared);
        stream_attributes->GetUINT32(MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, &types);
    }
    Check(SUCCEEDED(hr) && category == PINNAME_VIDEO_CAPTURE && shared == 1 &&
              types == MFFrameSourceTypes_Color,
          "stream attributes", "video capture pin, Frame Server shared, colour source");

    ComPtr<IKsControl> ks;
    ComPtr<IMFGetService> service;
    Check(SUCCEEDED(camera.source()->QueryInterface(IID_PPV_ARGS(&ks))) &&
              SUCCEEDED(camera.source()->QueryInterface(IID_PPV_ARGS(&service))),
          "source exposes IKsControl and IMFGetService");

    ComPtr<IMFPresentationDescriptor> presentation;
    ComPtr<IMFStreamDescriptor> descriptor;
    ComPtr<IMFMediaTypeHandler> handler;
    BOOL selected = FALSE;
    DWORD count = 0;
    camera.source()->CreatePresentationDescriptor(&presentation);
    presentation->GetStreamDescriptorByIndex(0, &selected, &descriptor);
    descriptor->GetMediaTypeHandler(&handler);
    handler->GetMediaTypeCount(&count);
    Check(count == 2, "two media types", "%lu", count);

    struct Expected {
        GUID subtype;
        const char* name;
        UINT32 stride;
        UINT32 size;
    } expected[] = {
        {MFVideoFormat_NV12, "NV12", PCVC_WIDTH, PCVC_NV12_SIZE},
        {MFVideoFormat_RGB32, "RGB32", PCVC_WIDTH * 4, static_cast<UINT32>(kRgbSize)},
    };
    for (DWORD i = 0; i < count && i < 2; ++i) {
        ComPtr<IMFMediaType> type;
        handler->GetMediaTypeByIndex(i, &type);
        GUID major = GUID_NULL;
        GUID subtype = GUID_NULL;
        UINT32 width = 0, height = 0, numerator = 0, denominator = 0, par_n = 0, par_d = 0;
        UINT32 interlace = 0, independent = 0, stride = 0, sample_size = 0, fixed = 0;
        type->GetGUID(MF_MT_MAJOR_TYPE, &major);
        type->GetGUID(MF_MT_SUBTYPE, &subtype);
        MFGetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, &width, &height);
        MFGetAttributeRatio(type.Get(), MF_MT_FRAME_RATE, &numerator, &denominator);
        MFGetAttributeRatio(type.Get(), MF_MT_PIXEL_ASPECT_RATIO, &par_n, &par_d);
        type->GetUINT32(MF_MT_INTERLACE_MODE, &interlace);
        type->GetUINT32(MF_MT_ALL_SAMPLES_INDEPENDENT, &independent);
        type->GetUINT32(MF_MT_DEFAULT_STRIDE, &stride);
        type->GetUINT32(MF_MT_SAMPLE_SIZE, &sample_size);
        type->GetUINT32(MF_MT_FIXED_SIZE_SAMPLES, &fixed);
        const bool ok = major == MFMediaType_Video && subtype == expected[i].subtype &&
                        width == PCVC_WIDTH && height == PCVC_HEIGHT &&
                        numerator == PCVC_FPS && denominator == 1 && par_n == 1 && par_d == 1 &&
                        interlace == MFVideoInterlace_Progressive && independent == 1 &&
                        stride == expected[i].stride && sample_size == expected[i].size &&
                        fixed == 1;
        Check(ok, "media type", "#%lu %s %ux%u @ %u/%u fps, PAR %u:%u, progressive, stride %u, "
              "sample %u bytes, independent", i, expected[i].name, width, height, numerator,
              denominator, par_n, par_d, stride, sample_size);
    }
    camera.Shutdown();
}

void TestPlaceholderAndTiming(DWORD type_index, bool provide_allocator) {
    const bool nv12 = type_index == 0;
    std::printf("\n== Placeholder and timing (%s, %s allocator, no PairCam app)\n",
                nv12 ? "NV12" : "RGB32", provide_allocator ? "Frame Server style" : "own");
    Camera camera;
    HRESULT hr = camera.Activate();
    if (SUCCEEDED(hr)) {
        hr = camera.Start(type_index, provide_allocator);
    }
    Check(SUCCEEDED(hr), "start", "hr=0x%08X", hr);
    if (FAILED(hr)) {
        camera.Shutdown();
        return;
    }

    std::vector<Sample> samples;
    const size_t wanted = 91; // about three seconds
    const bool pulled = camera.Pull(wanted, samples);
    Check(pulled, "pull samples", "%zu of %zu", samples.size(), wanted);
    if (samples.size() < 10) {
        camera.Shutdown();
        return;
    }

    const size_t size = nv12 ? PCVC_NV12_SIZE : kRgbSize;
    bool sizes_ok = true;
    bool all_placeholder = true;
    bool tokens_ok = true;
    for (const Sample& sample : samples) {
        sizes_ok &= sample.data.size() == size;
        all_placeholder &= nv12 ? IsPlaceholderNv12(sample.data) : IsPlaceholderRgb(sample.data);
        tokens_ok &= sample.has_token;
    }
    Check(sizes_ok, "sample size", "%zu bytes each", size);
    Check(tokens_ok, "every sample carries its request token");
    Check(all_placeholder, "placeholder when no app is running", "dark background in corners");
    if (nv12) {
        const size_t bright = BrightPixelsInCentre(samples.back().data);
        Check(true, "placeholder text", bright > 1000
                                            ? "wordmark and hint rendered (%zu light pixels)"
                                            : "plain dark frame (%zu light pixels)",
              bright);
    } else if (!samples.back().data.empty()) {
        const std::wstring path = SaveBitmap(samples.back().data, L"placeholder.bmp");
        Check(!path.empty(), "placeholder saved for a visual check", "%ls", path.c_str());
    }

    // Timestamps sit on an exact grid; deliveries arrive at a steady pace.
    bool grid = true;
    bool durations = true;
    for (size_t i = 1; i < samples.size(); ++i) {
        const LONGLONG step = samples[i].time - samples[i - 1].time;
        grid &= step == kFrameDuration || step == kFrameDuration + 1;
        durations &= samples[i].duration == kFrameDuration;
    }
    Check(grid, "sample times step by exactly 1/30 s", "333333 or 333334 x 100 ns");
    Check(durations, "sample durations", "333333 x 100 ns");

    // Skip the first few deliveries (thread start, placeholder rendering) when measuring.
    const size_t first = 5;
    const double span = samples.back().arrived_ms - samples[first].arrived_ms;
    const double fps = (samples.size() - 1 - first) * 1000.0 / span;
    double worst = 0;
    for (size_t i = first + 1; i < samples.size(); ++i) {
        worst = std::max(worst, std::fabs(samples[i].arrived_ms - samples[i - 1].arrived_ms -
                                          1000.0 / PCVC_FPS));
    }
    Check(fps > 29.5 && fps < 30.5, "steady 30 fps delivery", "%.2f fps, worst interval error "
          "%.1f ms", fps, worst);

    hr = camera.Stop();
    Check(SUCCEEDED(hr), "stop", "hr=0x%08X", hr);
    camera.Shutdown();
}

void TestFramesFromApp() {
    std::printf("\n== Frames from the app over the pipe\n");
    PipeServer server;
    Check(server.Listen(), "pipe server created", "%ls", kPipeName);

    Camera camera;
    HRESULT hr = camera.Activate();
    if (SUCCEEDED(hr)) {
        hr = camera.Start(0, true);
    }
    Check(SUCCEEDED(hr), "start NV12", "hr=0x%08X", hr);
    if (FAILED(hr)) {
        return;
    }

    const double connect_start = NowMs();
    const bool accepted = server.Accept(3000);
    Check(accepted, "DLL connects when a stream starts", "%.0f ms", NowMs() - connect_start);
    const int status = accepted ? server.ReadStatus(2000) : -1;
    Check(status == 1, "DLL reports streaming = 1", "%d", status);

    const std::vector<uint8_t> pattern = MakePattern(17);
    {
        FramePump pump(server, pattern);
        Sample found;
        const bool arrived = camera.PullUntil(
            [&](const Sample& sample) { return !IsPlaceholderNv12(sample.data); }, 3000, &found);
        Check(arrived, "app frames replace the placeholder");
        Check(arrived && found.data == pattern, "NV12 output is byte-identical to the app frame",
              "%zu bytes", found.data.size());

        // Several consecutive samples must all be the app frame (no flicker to placeholder).
        std::vector<Sample> samples;
        camera.Pull(30, samples);
        size_t identical = 0;
        for (const Sample& sample : samples) {
            identical += sample.data == pattern ? 1 : 0;
        }
        Check(identical == samples.size() && !samples.empty(), "steady app frames",
              "%zu of %zu identical", identical, samples.size());
    }

    // Idle message: placeholder right away.
    server.SendHeader(PCVC_TYPE_IDLE, 0);
    double start = NowMs();
    bool idle = camera.PullUntil([](const Sample& s) { return IsPlaceholderNv12(s.data); }, 2000);
    Check(idle, "IDLE message brings the placeholder back", "%.0f ms", NowMs() - start);

    // Frames, then silence: placeholder after PCVC_STALE_MS.
    server.SendFrame(pattern);
    camera.PullUntil([&](const Sample& s) { return s.data == pattern; }, 2000);
    start = NowMs();
    bool stale = camera.PullUntil([](const Sample& s) { return IsPlaceholderNv12(s.data); }, 3000);
    const double stale_ms = NowMs() - start;
    Check(stale && stale_ms > 800 && stale_ms < 1300, "placeholder after 1 s without frames",
          "%.0f ms", stale_ms);

    // Switch the running source to RGB32 (Start again with the other type).
    hr = camera.Stop();
    Check(SUCCEEDED(hr), "stop NV12");
    const int stopped = server.ReadStatus(2000);
    Check(stopped == 0, "DLL reports streaming = 0 when the stream stops", "%d", stopped);
    server.Close();

    Check(server.Listen(), "pipe server listens again");
    hr = camera.Start(1, true);
    Check(SUCCEEDED(hr), "start RGB32", "hr=0x%08X", hr);
    const bool again = server.Accept(3000) && server.ReadStatus(2000) == 1;
    Check(again, "DLL reconnects for the RGB32 stream");
    {
        FramePump pump(server, pattern);
        Sample found;
        const bool arrived = camera.PullUntil(
            [&](const Sample& sample) { return !IsPlaceholderRgb(sample.data); }, 3000, &found);
        Check(arrived && found.data.size() == kRgbSize, "RGB32 frames arrive", "%zu bytes",
              found.data.size());
        if (arrived && found.data.size() == kRgbSize) {
            int worst = 0;
            uint32_t seed = 12345;
            for (int i = 0; i < 20000; ++i) {
                seed = seed * 1664525u + 1013904223u;
                const uint32_t x = (seed >> 8) % PCVC_WIDTH;
                seed = seed * 1664525u + 1013904223u;
                const uint32_t y = (seed >> 8) % PCVC_HEIGHT;
                int expected[3];
                ExpectedRgb(pattern, x, y, expected);
                const uint8_t* pixel = found.data.data() + (static_cast<size_t>(y) * PCVC_WIDTH + x) * 4;
                // RGB32 is B, G, R, X in memory.
                worst = std::max({worst, std::abs(pixel[2] - expected[0]),
                                  std::abs(pixel[1] - expected[1]),
                                  std::abs(pixel[0] - expected[2])});
            }
            Check(worst <= 1, "RGB32 conversion matches BT.709 limited range",
                  "20000 random pixels, worst channel error %d", worst);
        }
    }
    camera.Stop();
    server.ReadStatus(2000);
    camera.Shutdown();
}

void TestRobustness() {
    std::printf("\n== Robustness of the pipe client\n");
    PipeServer server;
    server.Listen();
    Camera camera;
    HRESULT hr = camera.Activate();
    if (SUCCEEDED(hr)) {
        hr = camera.Start(0, false);
    }
    Check(SUCCEEDED(hr), "start", "hr=0x%08X", hr);
    if (FAILED(hr) || !server.Accept(3000) || server.ReadStatus(2000) != 1) {
        Check(false, "connect");
        camera.Shutdown();
        return;
    }

    const std::vector<uint8_t> pattern = MakePattern(99);

    // Zero-length writes, an unknown message type and a frame of the wrong size are skipped;
    // the connection survives and the next good frame shows.
    char nothing = 0;
    for (int i = 0; i < 3; ++i) {
        server.Transfer(true, &nothing, 0, 1000);
    }
    std::vector<uint8_t> junk(100, 0xAB);
    server.SendHeader(77, static_cast<uint32_t>(junk.size()));
    server.SendRaw(junk.data(), static_cast<DWORD>(junk.size()));
    std::vector<uint8_t> wrong_size(1280 * 720 * 3 / 2, 0x80);
    server.SendFrame(wrong_size, 1280, 720);
    server.SendFrame(pattern);
    bool shown = camera.PullUntil([&](const Sample& s) { return s.data == pattern; }, 2000);
    Check(shown, "zero-length writes, unknown types and wrong sizes are skipped");

    // The app dies mid-frame: half a frame, then the pipe closes.
    PcvcHeader header{PCVC_MAGIC, PCVC_VERSION, PCVC_TYPE_FRAME,
                      PCVC_FRAME_INFO_SIZE + PCVC_NV12_SIZE, 0};
    PcvcFrameInfo info{PCVC_WIDTH, PCVC_HEIGHT, 0};
    server.SendRaw(&header, sizeof(header));
    server.SendRaw(&info, sizeof(info));
    server.SendRaw(pattern.data(), PCVC_NV12_SIZE / 2);
    server.Close();
    bool placeholder =
        camera.PullUntil([](const Sample& s) { return IsPlaceholderNv12(s.data); }, 2000);
    Check(placeholder, "placeholder right after the app disconnects mid-frame");

    // The app comes back: the DLL reconnects by itself.
    server.Listen();
    double start = NowMs();
    const bool back = server.Accept(3000) && server.ReadStatus(2000) == 1;
    Check(back, "DLL reconnects when the app returns", "%.0f ms", NowMs() - start);

    // Garbage: a bad magic makes the DLL drop the connection and reconnect.
    server.SendHeader(PCVC_TYPE_FRAME, 16, 0xDEADBEEF);
    char probe[24];
    const bool dropped = !server.Transfer(false, probe, sizeof(probe), 2000);
    Check(dropped, "a bad magic drops the connection");
    server.Close();
    server.Listen();
    const bool recovered = server.Accept(3000) && server.ReadStatus(2000) == 1;
    Check(recovered, "and the DLL reconnects after garbage");
    {
        FramePump pump(server, pattern);
        shown = camera.PullUntil([&](const Sample& s) { return s.data == pattern; }, 2000);
    }
    Check(shown, "frames flow again after recovery");

    // Shutdown while streaming, with requests outstanding and the pipe connected.
    for (int i = 0; i < 4; ++i) {
        camera.stream()->RequestSample(nullptr);
    }
    start = NowMs();
    hr = camera.source()->Shutdown();
    const double shutdown_ms = NowMs() - start;
    const int status = server.ReadStatus(2000);
    Check(SUCCEEDED(hr) && shutdown_ms < 500, "shutdown while streaming is prompt",
          "%.1f ms", shutdown_ms);
    Check(status == 0, "shutdown reports streaming = 0", "%d", status);
    ComPtr<IMFPresentationDescriptor> descriptor;
    Check(camera.source()->CreatePresentationDescriptor(&descriptor) == MF_E_SHUTDOWN &&
              camera.stream()->RequestSample(nullptr) == MF_E_SHUTDOWN,
          "calls after shutdown return MF_E_SHUTDOWN");
    camera.Shutdown();
    server.Close();
}

void TestTwoConsumers() {
    std::printf("\n== Two sources in one process share one connection\n");
    PipeServer server;
    server.Listen();
    Camera first;
    Camera second;
    HRESULT hr = first.Activate();
    if (SUCCEEDED(hr)) {
        hr = second.Activate();
    }
    if (SUCCEEDED(hr)) {
        hr = first.Start(0, true);
    }
    if (SUCCEEDED(hr)) {
        hr = second.Start(1, true);
    }
    Check(SUCCEEDED(hr), "both start (NV12 and RGB32)", "hr=0x%08X", hr);
    const bool connected = server.Accept(3000) && server.ReadStatus(2000) == 1;
    Check(connected, "one connection");

    const std::vector<uint8_t> pattern = MakePattern(5);
    {
        FramePump pump(server, pattern);
        const bool a = first.PullUntil([&](const Sample& s) { return s.data == pattern; }, 3000);
        const bool b = second.PullUntil(
            [&](const Sample& s) { return s.data.size() == kRgbSize && !IsPlaceholderRgb(s.data); },
            3000);
        Check(a && b, "both streams show the app frame");

        first.Stop();
        const int early = server.ReadStatus(400);
        Check(early == -1, "stopping one stream keeps the camera in use");
        const bool still = second.PullUntil(
            [&](const Sample& s) { return s.data.size() == kRgbSize && !IsPlaceholderRgb(s.data); },
            2000);
        Check(still, "the other stream keeps running");
    }
    second.Stop();
    const int last = server.ReadStatus(2000);
    Check(last == 0, "stopping the last stream reports streaming = 0", "%d", last);
    first.Shutdown();
    second.Shutdown();
}

void TestNoLeaks() {
    std::printf("\n== Repeated start, stop and shutdown\n");
    // One warm-up cycle so Media Foundation's own worker threads exist before measuring.
    auto cycle = [](bool with_server) {
        PipeServer server;
        if (with_server) {
            server.Listen();
        }
        Camera camera;
        if (FAILED(camera.Activate()) || FAILED(camera.Start(0, true))) {
            return false;
        }
        if (with_server && (!server.Accept(3000) || server.ReadStatus(2000) != 1)) {
            return false;
        }
        std::vector<Sample> samples;
        const bool ok = camera.Pull(5, samples);
        camera.Stop();
        camera.Shutdown();
        return ok;
    };
    cycle(true);
    Sleep(200);
    const DWORD threads_before = ThreadCount();
    const DWORD handles_before = HandleCount();
    bool all = true;
    for (int i = 0; i < 20; ++i) {
        all &= cycle(i % 2 == 0);
    }
    Sleep(300);
    const DWORD threads_after = ThreadCount();
    const DWORD handles_after = HandleCount();
    Check(all, "20 cycles of activate, start, pull, stop, shutdown");
    Check(threads_after <= threads_before, "no leaked threads", "%lu before, %lu after",
          threads_before, threads_after);
    Check(handles_after <= handles_before + 8, "no leaked handles", "%lu before, %lu after",
          handles_before, handles_after);
}

// Run by the Rust test `vcam::tests::end_to_end_with_the_dll`: the real PairCam pipe server
// (crates/engine/src/vcam.rs) is the app and sends a flat grey portrait picture (Y 200, neutral
// chroma, 1080x1920), which must arrive letterboxed: grey in the middle, black bars at the sides.
void TestExternalApp() {
    std::printf("\n== Frames from the Rust pipe server\n");
    for (DWORD type_index : {0u, 1u}) {
        const bool nv12 = type_index == 0;
        Camera camera;
        HRESULT hr = camera.Activate();
        if (SUCCEEDED(hr)) {
            hr = camera.Start(type_index, true);
        }
        Check(SUCCEEDED(hr), nv12 ? "start NV12" : "start RGB32", "hr=0x%08X", hr);
        if (FAILED(hr)) {
            camera.Shutdown();
            continue;
        }

        auto value = [&](const Sample& sample, uint32_t x, uint32_t y) -> int {
            if (nv12) {
                return sample.data[static_cast<size_t>(y) * PCVC_WIDTH + x];
            }
            return sample.data[(static_cast<size_t>(y) * PCVC_WIDTH + x) * 4 + 1]; // green
        };
        Sample found;
        const bool arrived = camera.PullUntil(
            [&](const Sample& sample) {
                return sample.data.size() == (nv12 ? PCVC_NV12_SIZE : kRgbSize) &&
                       !(nv12 ? IsPlaceholderNv12(sample.data) : IsPlaceholderRgb(sample.data));
            },
            5000, &found);
        Check(arrived, "app frames arrive through the Rust server");
        if (arrived) {
            const int centre = value(found, 960, 540);
            const int left_bar = value(found, 100, 540);
            const int right_bar = value(found, 1800, 540);
            const int inside_edge = value(found, 660, 540);
            const int grey = nv12 ? 200 : 214;
            const int black = nv12 ? 16 : 0;
            Check(std::abs(centre - grey) <= 2 && std::abs(inside_edge - grey) <= 6 &&
                      left_bar == black && right_bar == black,
                  "portrait video is letterboxed",
                  "centre %d, bars %d and %d, picture edge %d", centre, left_bar, right_bar,
                  inside_edge);
        }
        camera.Stop();
        camera.Shutdown();
    }
}

} // namespace

int wmain(int argc, wchar_t** argv) {
    bool external_app = false;
    std::wstring path;
    for (int i = 1; i < argc; ++i) {
        if (std::wstring(argv[i]) == L"--external-app") {
            external_app = true;
        } else {
            path = argv[i];
        }
    }
    if (path.empty()) {
        wchar_t self[MAX_PATH];
        GetModuleFileNameW(nullptr, self, MAX_PATH);
        path = self;
        path = path.substr(0, path.find_last_of(L'\\') + 1) + L"paircam_vcam.dll";
    }

    // 1 ms timer resolution so the polling loops measure delivery times, not the 15.6 ms
    // default scheduler tick.
    timeBeginPeriod(1);
    CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    MFStartup(MF_VERSION);

    std::printf("PairCam virtual camera tests\nDLL: %ls\n", path.c_str());
    HMODULE module = LoadLibraryW(path.c_str());
    if (module == nullptr) {
        std::printf("FAIL  cannot load the DLL (error %lu)\n", GetLastError());
        return 1;
    }
    g_get_class_object =
        reinterpret_cast<GetClassObjectFunction>(GetProcAddress(module, "DllGetClassObject"));
    g_can_unload_now =
        reinterpret_cast<CanUnloadNowFunction>(GetProcAddress(module, "DllCanUnloadNow"));
    Check(g_get_class_object != nullptr && g_can_unload_now != nullptr &&
              GetProcAddress(module, "DllRegisterServer") != nullptr &&
              GetProcAddress(module, "DllUnregisterServer") != nullptr,
          "COM server exports");
    if (g_get_class_object == nullptr || g_can_unload_now == nullptr) {
        return 1;
    }
    ComPtr<IClassFactory> wrong;
    Check(g_get_class_object(GUID_NULL, IID_PPV_ARGS(&wrong)) == CLASS_E_CLASSNOTAVAILABLE,
          "unknown CLSID is refused");

    if (external_app) {
        TestExternalApp();
    } else {
        TestActivationAndMediaTypes();
        TestPlaceholderAndTiming(0, true);
        TestPlaceholderAndTiming(1, false);
        TestFramesFromApp();
        TestRobustness();
        TestTwoConsumers();
        TestNoLeaks();
    }

    std::printf("\n== Unload\n");
    Sleep(100);
    Check(g_can_unload_now() == S_OK, "DllCanUnloadNow returns S_OK once everything is released");

    MFShutdown();
    CoUninitialize();
    std::printf("\n%d passed, %d failed\n", g_passes, g_failures);
    return g_failures;
}
