// Portions adapted from smourier/VCamSample (MediaStream.cpp), MIT License,
// Copyright (c) 2024-2026 Simon Mourier. See the full notice in media_source.cpp.

#include "media_stream.h"

#include "convert.h"
#include "frame_feed.h"
#include "placeholder.h"

namespace {

// One frame at PCVC_FPS, in Media Foundation's 100 ns units.
constexpr LONGLONG kSecond = 10'000'000;
constexpr LONGLONG kFrameDuration = kSecond / PCVC_FPS;

// Samples the allocator keeps. Frame Server holds a few while it fans frames out to apps.
constexpr DWORD kAllocatorSamples = 8;

// A consumer that stops answering would otherwise grow the request queue without bound.
constexpr size_t kMaxTokens = 30;

HRESULT CreateMediaType(const GUID& subtype, IMFMediaType** result) {
    const bool nv12 = subtype == MFVideoFormat_NV12;
    const UINT32 sample_size = nv12 ? PCVC_NV12_SIZE : PCVC_WIDTH * PCVC_HEIGHT * 4;

    ComPtr<IMFMediaType> type;
    PC_RETURN_IF_FAILED(MFCreateMediaType(&type));
    PC_RETURN_IF_FAILED(type->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video));
    PC_RETURN_IF_FAILED(type->SetGUID(MF_MT_SUBTYPE, subtype));
    PC_RETURN_IF_FAILED(MFSetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, PCVC_WIDTH, PCVC_HEIGHT));
    PC_RETURN_IF_FAILED(MFSetAttributeRatio(type.Get(), MF_MT_FRAME_RATE, PCVC_FPS, 1));
    PC_RETURN_IF_FAILED(MFSetAttributeRatio(type.Get(), MF_MT_FRAME_RATE_RANGE_MIN, PCVC_FPS, 1));
    PC_RETURN_IF_FAILED(MFSetAttributeRatio(type.Get(), MF_MT_FRAME_RATE_RANGE_MAX, PCVC_FPS, 1));
    PC_RETURN_IF_FAILED(MFSetAttributeRatio(type.Get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1));
    PC_RETURN_IF_FAILED(type->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive));
    PC_RETURN_IF_FAILED(type->SetUINT32(MF_MT_ALL_SAMPLES_INDEPENDENT, TRUE));
    PC_RETURN_IF_FAILED(type->SetUINT32(MF_MT_FIXED_SIZE_SAMPLES, TRUE));
    PC_RETURN_IF_FAILED(type->SetUINT32(MF_MT_SAMPLE_SIZE, sample_size));
    // The stride of the first plane, positive because rows are top-down. For NV12 that is one
    // byte per pixel (VCamSample's width * 1.5 is a known mistake); for RGB32 four.
    PC_RETURN_IF_FAILED(type->SetUINT32(MF_MT_DEFAULT_STRIDE, nv12 ? PCVC_WIDTH : PCVC_WIDTH * 4));
    PC_RETURN_IF_FAILED(type->SetUINT32(MF_MT_AVG_BITRATE, sample_size * 8 * PCVC_FPS));
    PC_RETURN_IF_FAILED(type->SetUINT32(MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709));
    PC_RETURN_IF_FAILED(type->SetUINT32(MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709));
    if (nv12) {
        PC_RETURN_IF_FAILED(type->SetUINT32(MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709));
        PC_RETURN_IF_FAILED(type->SetUINT32(MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235));
    } else {
        PC_RETURN_IF_FAILED(type->SetUINT32(MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_0_255));
    }
    *result = type.Detach();
    return S_OK;
}

HRESULT ValidateMediaType(IMFMediaType* type, GUID* subtype) {
    GUID major = GUID_NULL;
    PC_RETURN_IF_FAILED(type->GetGUID(MF_MT_MAJOR_TYPE, &major));
    PC_RETURN_IF_FAILED(type->GetGUID(MF_MT_SUBTYPE, subtype));
    UINT32 width = 0;
    UINT32 height = 0;
    PC_RETURN_IF_FAILED(MFGetAttributeSize(type, MF_MT_FRAME_SIZE, &width, &height));
    if (major != MFMediaType_Video ||
        (*subtype != MFVideoFormat_NV12 && *subtype != MFVideoFormat_RGB32) ||
        width != PCVC_WIDTH || height != PCVC_HEIGHT) {
        return MF_E_INVALIDMEDIATYPE;
    }
    return S_OK;
}

} // namespace

HRESULT MediaStream::Create(IMFMediaSource* source, DWORD id, MediaStream** stream) {
    *stream = nullptr;
    auto* created = new (std::nothrow) MediaStream();
    if (created == nullptr) {
        return E_OUTOFMEMORY;
    }
    const HRESULT result = created->Initialize(source, id);
    if (FAILED(result)) {
        created->Release();
        return result;
    }
    *stream = created;
    return S_OK;
}

HRESULT MediaStream::Initialize(IMFMediaSource* source, DWORD id) {
    source_ = source;
    id_ = id;

    // The attributes Frame Server looks for on a camera stream (same set as VCamSample and
    // Microsoft's VirtualCamera sample).
    PC_RETURN_IF_FAILED(InitializeAttributes());
    PC_RETURN_IF_FAILED(SetGUID(MF_DEVICESTREAM_STREAM_CATEGORY, PINNAME_VIDEO_CAPTURE));
    PC_RETURN_IF_FAILED(SetUINT32(MF_DEVICESTREAM_STREAM_ID, id));
    PC_RETURN_IF_FAILED(SetUINT32(MF_DEVICESTREAM_FRAMESERVER_SHARED, 1));
    PC_RETURN_IF_FAILED(
        SetUINT32(MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color));

    PC_RETURN_IF_FAILED(MFCreateEventQueue(&queue_));

    // NV12 first: it is what the app sends, so the preferred type costs a single copy. RGB32
    // follows for apps that ask for it, converted on the CPU.
    ComPtr<IMFMediaType> nv12;
    ComPtr<IMFMediaType> rgb32;
    PC_RETURN_IF_FAILED(CreateMediaType(MFVideoFormat_NV12, &nv12));
    PC_RETURN_IF_FAILED(CreateMediaType(MFVideoFormat_RGB32, &rgb32));
    IMFMediaType* types[] = {nv12.Get(), rgb32.Get()};
    PC_RETURN_IF_FAILED(MFCreateStreamDescriptor(id, ARRAYSIZE(types), types, &descriptor_));

    ComPtr<IMFMediaTypeHandler> handler;
    PC_RETURN_IF_FAILED(descriptor_->GetMediaTypeHandler(&handler));
    PC_RETURN_IF_FAILED(handler->SetCurrentMediaType(nv12.Get()));
    return S_OK;
}

MediaStream::~MediaStream() {
    // Normally Shutdown already did this; it is safe to repeat.
    StopDelivery();
    if (consumer_.exchange(false)) {
        FrameFeed::Instance().RemoveConsumer();
    }
}

// IUnknown

STDMETHODIMP MediaStream::QueryInterface(REFIID riid, void** object) {
    if (object == nullptr) {
        return E_POINTER;
    }
    if (riid == __uuidof(IUnknown) || riid == __uuidof(IMFMediaEventGenerator) ||
        riid == __uuidof(IMFMediaStream) || riid == __uuidof(IMFMediaStream2)) {
        *object = static_cast<IMFMediaStream2*>(this);
    } else if (riid == __uuidof(IMFAttributes)) {
        *object = static_cast<IMFAttributes*>(this);
    } else if (riid == __uuidof(IKsControl)) {
        *object = static_cast<IKsControl*>(this);
    } else {
        *object = nullptr;
        return E_NOINTERFACE;
    }
    AddRef();
    return S_OK;
}

STDMETHODIMP_(ULONG) MediaStream::AddRef() { return references_.fetch_add(1) + 1; }

STDMETHODIMP_(ULONG) MediaStream::Release() {
    const ULONG remaining = references_.fetch_sub(1) - 1;
    if (remaining == 0) {
        delete this;
    }
    return remaining;
}

// IMFMediaEventGenerator. The queue is copied out under the lock and used without it:
// GetEvent can block for as long as the caller likes.

STDMETHODIMP MediaStream::BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) {
    ComPtr<IMFMediaEventQueue> queue;
    {
        ExclusiveGuard guard(lock_);
        queue = queue_;
    }
    return queue ? queue->BeginGetEvent(callback, state) : MF_E_SHUTDOWN;
}

STDMETHODIMP MediaStream::EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) {
    if (event == nullptr) {
        return E_POINTER;
    }
    *event = nullptr;
    ComPtr<IMFMediaEventQueue> queue;
    {
        ExclusiveGuard guard(lock_);
        queue = queue_;
    }
    return queue ? queue->EndGetEvent(result, event) : MF_E_SHUTDOWN;
}

STDMETHODIMP MediaStream::GetEvent(DWORD flags, IMFMediaEvent** event) {
    if (event == nullptr) {
        return E_POINTER;
    }
    *event = nullptr;
    ComPtr<IMFMediaEventQueue> queue;
    {
        ExclusiveGuard guard(lock_);
        queue = queue_;
    }
    return queue ? queue->GetEvent(flags, event) : MF_E_SHUTDOWN;
}

STDMETHODIMP MediaStream::QueueEvent(MediaEventType type, REFGUID extended_type, HRESULT status,
                                     const PROPVARIANT* value) {
    ComPtr<IMFMediaEventQueue> queue;
    {
        ExclusiveGuard guard(lock_);
        queue = queue_;
    }
    return queue ? queue->QueueEventParamVar(type, extended_type, status, value) : MF_E_SHUTDOWN;
}

// IMFMediaStream

STDMETHODIMP MediaStream::GetMediaSource(IMFMediaSource** source) {
    if (source == nullptr) {
        return E_POINTER;
    }
    *source = nullptr;
    ExclusiveGuard guard(lock_);
    if (shut_down_ || !source_) {
        return MF_E_SHUTDOWN;
    }
    return source_.CopyTo(source);
}

STDMETHODIMP MediaStream::GetStreamDescriptor(IMFStreamDescriptor** descriptor) {
    if (descriptor == nullptr) {
        return E_POINTER;
    }
    *descriptor = nullptr;
    ExclusiveGuard guard(lock_);
    if (shut_down_ || !descriptor_) {
        return MF_E_SHUTDOWN;
    }
    return descriptor_.CopyTo(descriptor);
}

STDMETHODIMP MediaStream::RequestSample(IUnknown* token) {
    ExclusiveGuard guard(lock_);
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }
    if (state_ == MF_STREAM_STATE_STOPPED) {
        return MF_E_INVALIDREQUEST;
    }
    if (tokens_.size() >= kMaxTokens) {
        tokens_.pop_front();
    }
    try {
        tokens_.emplace_back(token);
    } catch (const std::bad_alloc&) {
        return E_OUTOFMEMORY;
    }
    return S_OK;
}

// IMFMediaStream2

STDMETHODIMP MediaStream::SetStreamState(MF_STREAM_STATE state) {
    {
        ExclusiveGuard control(control_lock_);
        ExclusiveGuard guard(lock_);
        if (shut_down_) {
            return MF_E_SHUTDOWN;
        }
        const MF_STREAM_STATE current = state_;
        if (current == state) {
            return S_OK;
        }
        // Pausing and resuming only gate delivery; the thread keeps its clock.
        if (state == MF_STREAM_STATE_PAUSED) {
            if (current != MF_STREAM_STATE_RUNNING) {
                return MF_E_INVALID_STATE_TRANSITION;
            }
            state_ = MF_STREAM_STATE_PAUSED;
            return S_OK;
        }
        if (state == MF_STREAM_STATE_RUNNING && current == MF_STREAM_STATE_PAUSED) {
            state_ = MF_STREAM_STATE_RUNNING;
            return S_OK;
        }
    }

    // Start and Stop take the control lock themselves.
    switch (state) {
    case MF_STREAM_STATE_RUNNING:
        return Start(nullptr);
    case MF_STREAM_STATE_STOPPED:
        return Stop();
    default:
        return MF_E_INVALID_STATE_TRANSITION;
    }
}

STDMETHODIMP MediaStream::GetStreamState(MF_STREAM_STATE* state) {
    if (state == nullptr) {
        return E_POINTER;
    }
    ExclusiveGuard guard(lock_);
    *state = state_;
    return S_OK;
}

// IKsControl. PairCam exposes no camera controls to apps yet (zoom, exposure and so on are
// driven from the PairCam app), so every property set is reported as absent, which is what
// Frame Server expects from a camera without them.

STDMETHODIMP_(NTSTATUS) MediaStream::KsProperty(PKSPROPERTY, ULONG, void*, ULONG,
                                               ULONG* returned) {
    if (returned != nullptr) {
        *returned = 0;
    }
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

STDMETHODIMP_(NTSTATUS) MediaStream::KsMethod(PKSMETHOD, ULONG, void*, ULONG, ULONG* returned) {
    if (returned != nullptr) {
        *returned = 0;
    }
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

STDMETHODIMP_(NTSTATUS) MediaStream::KsEvent(PKSEVENT, ULONG, void*, ULONG, ULONG* returned) {
    if (returned != nullptr) {
        *returned = 0;
    }
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

// Control, called by the media source

bool MediaStream::IsActive() {
    ExclusiveGuard guard(lock_);
    return !shut_down_ && state_ != MF_STREAM_STATE_STOPPED;
}

HRESULT MediaStream::SetAllocator(IUnknown* allocator) {
    if (allocator == nullptr) {
        return E_POINTER;
    }
    ComPtr<IMFVideoSampleAllocatorEx> provided;
    PC_RETURN_IF_FAILED(allocator->QueryInterface(IID_PPV_ARGS(&provided)));
    ExclusiveGuard guard(lock_);
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }
    allocator_ = provided;
    return S_OK;
}

HRESULT MediaStream::Start(IMFMediaType* type) {
    ExclusiveGuard control(control_lock_);

    ComPtr<IMFMediaType> effective(type);
    if (!effective) {
        ComPtr<IMFStreamDescriptor> descriptor;
        PC_RETURN_IF_FAILED(GetStreamDescriptor(&descriptor));
        ComPtr<IMFMediaTypeHandler> handler;
        PC_RETURN_IF_FAILED(descriptor->GetMediaTypeHandler(&handler));
        PC_RETURN_IF_FAILED(handler->GetCurrentMediaType(&effective));
    }
    GUID subtype = GUID_NULL;
    PC_RETURN_IF_FAILED(ValidateMediaType(effective.Get(), &subtype));

    // A restart (new format, or Start while running) begins from a clean delivery thread.
    StopDelivery();

    {
        ExclusiveGuard guard(lock_);
        if (shut_down_) {
            return MF_E_SHUTDOWN;
        }
        if (!allocator_) {
            PC_RETURN_IF_FAILED(MFCreateVideoSampleAllocatorEx(IID_PPV_ARGS(&allocator_)));
        }
        // Frame Server may hand us a D3D manager; this stream deliberately leaves the allocator
        // without one, so samples are always in system memory. That is the path Chrome, Edge
        // and Teams use anyway, and it keeps one tested code path. Frame Server uploads to the
        // GPU for apps that want textures.
        allocator_->UninitializeSampleAllocator();
        PC_RETURN_IF_FAILED(
            allocator_->InitializeSampleAllocator(kAllocatorSamples, effective.Get()));
        format_ = subtype;
        state_ = MF_STREAM_STATE_RUNNING;
        PC_RETURN_IF_FAILED(queue_->QueueEventParamVar(MEStreamStarted, GUID_NULL, S_OK, nullptr));
    }

    if (!consumer_.exchange(true)) {
        FrameFeed::Instance().AddConsumer();
    }
    StartDelivery();
    return S_OK;
}

HRESULT MediaStream::Stop() {
    ExclusiveGuard control(control_lock_);
    StopDelivery();
    if (consumer_.exchange(false)) {
        FrameFeed::Instance().RemoveConsumer();
    }

    ExclusiveGuard guard(lock_);
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }
    state_ = MF_STREAM_STATE_STOPPED;
    tokens_.clear();
    if (allocator_) {
        allocator_->UninitializeSampleAllocator();
    }
    return queue_->QueueEventParamVar(MEStreamStopped, GUID_NULL, S_OK, nullptr);
}

void MediaStream::Shutdown() {
    ExclusiveGuard control(control_lock_);
    StopDelivery();
    if (consumer_.exchange(false)) {
        FrameFeed::Instance().RemoveConsumer();
    }

    ComPtr<IMFMediaEventQueue> queue;
    ComPtr<IMFMediaSource> source;
    {
        ExclusiveGuard guard(lock_);
        if (shut_down_) {
            return;
        }
        shut_down_ = true;
        state_ = MF_STREAM_STATE_STOPPED;
        tokens_.clear();
        if (allocator_) {
            allocator_->UninitializeSampleAllocator();
            allocator_.Reset();
        }
        queue.Swap(queue_);
        // Breaks the source <-> stream reference cycle.
        source.Swap(source_);
        descriptor_.Reset();
    }
    // Outside the lock: shutting the queue down wakes anyone blocked in GetEvent, and releasing
    // the source may run its destructor.
    if (queue) {
        queue->Shutdown();
    }
}

// Delivery

void MediaStream::StartDelivery() {
    delivery_stop_.reset(CreateEventW(nullptr, TRUE, FALSE, nullptr));
    if (!delivery_stop_.valid()) {
        return;
    }
    delivery_thread_.reset(CreateThread(nullptr, 0, DeliveryMain, this, 0, nullptr));
    if (!delivery_thread_.valid()) {
        delivery_stop_.reset();
    }
}

void MediaStream::StopDelivery() {
    if (delivery_thread_.valid()) {
        SetEvent(delivery_stop_.get());
        WaitForSingleObject(delivery_thread_.get(), INFINITE);
    }
    delivery_thread_.reset();
    delivery_stop_.reset();
}

DWORD WINAPI MediaStream::DeliveryMain(void* context) {
    // The stream outlives this thread: StopDelivery joins it before the stream can go away.
    const HRESULT com = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    static_cast<MediaStream*>(context)->DeliveryLoop();
    if (SUCCEEDED(com)) {
        CoUninitialize();
    }
    return 0;
}

void MediaStream::DeliveryLoop() {
    // Render the placeholder here, on our own thread, before the first tick.
    Placeholder();

    // A high-resolution timer keeps ticks within about a millisecond of the grid. Older
    // systems without the flag get a normal waitable timer.
    UniqueHandle timer(CreateWaitableTimerExW(nullptr, nullptr,
                                              CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                                              TIMER_ALL_ACCESS));
    if (!timer.valid()) {
        timer.reset(CreateWaitableTimerExW(nullptr, nullptr, 0, TIMER_ALL_ACCESS));
    }
    if (!timer.valid()) {
        return;
    }
    const HANDLE stop = delivery_stop_.get();

    // Sample times sit on a regular grid: start + n * (1 s / PCVC_FPS), on Media Foundation's
    // presentation clock (MFGetSystemTime). Computing each tick from the start avoids drift.
    const LONGLONG start = MFGetSystemTime();
    LONGLONG index = 1;
    for (;;) {
        LONGLONG due = start + index * kSecond / PCVC_FPS;
        LONGLONG now = MFGetSystemTime();
        if (now - due > 2 * kFrameDuration) {
            // We fell behind (the machine slept, or the process was suspended): skip the missed
            // ticks instead of bursting to catch up.
            index = (now - start) * PCVC_FPS / kSecond + 1;
            due = start + index * kSecond / PCVC_FPS;
        }
        if (due > now) {
            LARGE_INTEGER relative;
            relative.QuadPart = -(due - now);
            SetWaitableTimer(timer.get(), &relative, 0, nullptr, nullptr, FALSE);
            const HANDLE handles[2] = {stop, timer.get()};
            if (WaitForMultipleObjects(2, handles, FALSE, INFINITE) != WAIT_OBJECT_0 + 1) {
                break;
            }
        } else if (WaitForSingleObject(stop, 0) == WAIT_OBJECT_0) {
            break;
        }
        DeliverOne(due);
        ++index;
    }
    CancelWaitableTimer(timer.get());
}

void MediaStream::DeliverOne(LONGLONG time) {
    ComPtr<IMFVideoSampleAllocatorEx> allocator;
    ComPtr<IMFMediaEventQueue> queue;
    GUID format;
    {
        ExclusiveGuard guard(lock_);
        if (shut_down_ || state_ != MF_STREAM_STATE_RUNNING || tokens_.empty() || !allocator_) {
            return; // Nobody asked for this tick (or we are paused); skip it.
        }
        allocator = allocator_;
        queue = queue_;
        format = format_;
    }

    // Allocation and the frame copy happen without the lock. If the allocator is empty (the
    // consumer still holds every sample), the request stays queued for the next tick.
    ComPtr<IMFSample> sample;
    if (FAILED(allocator->AllocateSample(&sample)) || FAILED(FillSample(sample.Get(), format)) ||
        FAILED(sample->SetSampleTime(time)) ||
        FAILED(sample->SetSampleDuration(kFrameDuration)) ||
        FAILED(sample->SetUINT32(MFSampleExtension_CleanPoint, TRUE))) {
        return;
    }

    ComPtr<IUnknown> token;
    {
        ExclusiveGuard guard(lock_);
        if (shut_down_ || state_ != MF_STREAM_STATE_RUNNING || tokens_.empty()) {
            return;
        }
        token = std::move(tokens_.front());
        tokens_.pop_front();
    }
    if (token && FAILED(sample->SetUnknown(MFSampleExtension_Token, token.Get()))) {
        return;
    }
    queue->QueueEventParamUnk(MEMediaSample, GUID_NULL, S_OK, sample.Get());
}

HRESULT MediaStream::FillSample(IMFSample* sample, const GUID& format) {
    const bool nv12 = format == MFVideoFormat_NV12;
    const DWORD size = nv12 ? PCVC_NV12_SIZE : PCVC_WIDTH * PCVC_HEIGHT * 4;
    const PlaceholderPicture& placeholder = Placeholder();

    ComPtr<IMFMediaBuffer> buffer;
    PC_RETURN_IF_FAILED(sample->GetBufferByIndex(0, &buffer));

    ComPtr<IMF2DBuffer2> buffer_2d;
    if (SUCCEEDED(buffer.As(&buffer_2d))) {
        BYTE* scanline0 = nullptr;
        LONG pitch = 0;
        BYTE* start = nullptr;
        DWORD length = 0;
        PC_RETURN_IF_FAILED(buffer_2d->Lock2DSize(MF2DBuffer_LockFlags_Write, &scanline0, &pitch,
                                                  &start, &length));

        // Check that every row we write lies inside the locked buffer. NV12 planes are never
        // bottom-up; RGB32 rows may be (negative pitch).
        const LONG row_bytes = static_cast<LONG>(nv12 ? PCVC_WIDTH : PCVC_WIDTH * 4);
        const LONGLONG rows = nv12 ? PCVC_HEIGHT * 3 / 2 : PCVC_HEIGHT;
        const LONGLONG first = scanline0 - start;
        const LONGLONG last = first + (rows - 1) * pitch;
        const LONGLONG low = first < last ? first : last;
        const LONGLONG high = (first < last ? last : first) + row_bytes;
        if ((nv12 && pitch < row_bytes) || (!nv12 && (pitch < 0 ? -pitch : pitch) < row_bytes) ||
            low < 0 || high > static_cast<LONGLONG>(length)) {
            buffer_2d->Unlock2D();
            return MF_E_BUFFERTOOSMALL;
        }

        // In a Media Foundation NV12 buffer the UV plane follows the Y rows at the same pitch.
        BYTE* uv = scanline0 + static_cast<ptrdiff_t>(pitch) * PCVC_HEIGHT;
        const bool fresh = FrameFeed::Instance().WithNewest([&](const uint8_t* frame) {
            if (nv12) {
                CopyNv12(frame, PCVC_WIDTH, PCVC_HEIGHT, scanline0, pitch, uv, pitch);
            } else {
                Nv12ToRgb32(frame, PCVC_WIDTH, PCVC_HEIGHT, scanline0, pitch);
            }
        });
        if (!fresh) {
            if (nv12) {
                CopyNv12(placeholder.nv12.data(), PCVC_WIDTH, PCVC_HEIGHT, scanline0, pitch, uv,
                         pitch);
            } else {
                CopyRgb32(placeholder.bgra.data(), PCVC_WIDTH, PCVC_HEIGHT, scanline0, pitch);
            }
        }
        PC_RETURN_IF_FAILED(buffer_2d->Unlock2D());

        // A 2D buffer reports its length in the contiguous (tightly packed) layout.
        DWORD contiguous = 0;
        PC_RETURN_IF_FAILED(buffer_2d->GetContiguousLength(&contiguous));
        return buffer->SetCurrentLength(contiguous);
    } else {
        // A plain buffer: tightly packed, top-down, as the media type's default stride says.
        BYTE* data = nullptr;
        DWORD capacity = 0;
        PC_RETURN_IF_FAILED(buffer->Lock(&data, &capacity, nullptr));
        if (capacity < size) {
            buffer->Unlock();
            return MF_E_BUFFERTOOSMALL;
        }
        uint8_t* uv = data + PCVC_WIDTH * PCVC_HEIGHT;
        const bool fresh = FrameFeed::Instance().WithNewest([&](const uint8_t* frame) {
            if (nv12) {
                CopyNv12(frame, PCVC_WIDTH, PCVC_HEIGHT, data, PCVC_WIDTH, uv, PCVC_WIDTH);
            } else {
                Nv12ToRgb32(frame, PCVC_WIDTH, PCVC_HEIGHT, data, PCVC_WIDTH * 4);
            }
        });
        if (!fresh) {
            if (nv12) {
                CopyNv12(placeholder.nv12.data(), PCVC_WIDTH, PCVC_HEIGHT, data, PCVC_WIDTH, uv,
                         PCVC_WIDTH);
            } else {
                CopyRgb32(placeholder.bgra.data(), PCVC_WIDTH, PCVC_HEIGHT, data,
                          PCVC_WIDTH * 4);
            }
        }
        PC_RETURN_IF_FAILED(buffer->Unlock());
    }
    return buffer->SetCurrentLength(size);
}
