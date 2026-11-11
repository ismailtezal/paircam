// The structure of this media source, its stream attributes, the sensor profile and the
// activator follow smourier/VCamSample, used under the MIT License:
//
//   MIT License
//
//   Copyright (c) 2024-2026 Simon Mourier
//
//   Permission is hereby granted, free of charge, to any person obtaining a copy
//   of this software and associated documentation files (the "Software"), to deal
//   in the Software without restriction, including without limitation the rights
//   to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//   copies of the Software, and to permit persons to whom the Software is
//   furnished to do so, subject to the following conditions:
//
//   The above copyright notice and this permission notice shall be included in all
//   copies or substantial portions of the Software.
//
//   THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//   IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//   FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//   AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//   LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//   OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//   SOFTWARE.

#include "media_source.h"

#include <mfidl.h>

HRESULT MediaSource::Create(IMFAttributes* activator_attributes, MediaSource** source) {
    *source = nullptr;
    auto* created = new (std::nothrow) MediaSource();
    if (created == nullptr) {
        return E_OUTOFMEMORY;
    }
    const HRESULT result = created->Initialize(activator_attributes);
    if (FAILED(result)) {
        created->Shutdown();
        created->Release();
        return result;
    }
    *source = created;
    return S_OK;
}

HRESULT MediaSource::Initialize(IMFAttributes* activator_attributes) {
    PC_RETURN_IF_FAILED(InitializeAttributes());
    if (activator_attributes != nullptr) {
        PC_RETURN_IF_FAILED(activator_attributes->CopyAllItems(attributes_.Get()));
    }

    // One "legacy" sensor profile that admits our single 30 fps mode. Without a profile
    // collection some capture apps (the Windows Camera app among them) find no usable mode.
    ComPtr<IMFSensorProfileCollection> profiles;
    PC_RETURN_IF_FAILED(MFCreateSensorProfileCollection(&profiles));
    ComPtr<IMFSensorProfile> profile;
    PC_RETURN_IF_FAILED(MFCreateSensorProfile(KSCAMERAPROFILE_Legacy, 0, nullptr, &profile));
    PC_RETURN_IF_FAILED(profile->AddProfileFilter(0, L"((RES==;FRT<=30,1;SUT==))"));
    PC_RETURN_IF_FAILED(profiles->AddProfile(profile.Get()));
    PC_RETURN_IF_FAILED(SetUnknown(MF_DEVICEMFT_SENSORPROFILE_COLLECTION, profiles.Get()));

    ComPtr<MediaStream> stream;
    PC_RETURN_IF_FAILED(MediaStream::Create(this, 0, &stream));
    ComPtr<IMFStreamDescriptor> stream_descriptor;
    PC_RETURN_IF_FAILED(stream->GetStreamDescriptor(&stream_descriptor));
    try {
        streams_.push_back(stream);
    } catch (const std::bad_alloc&) {
        stream->Shutdown();
        return E_OUTOFMEMORY;
    }

    IMFStreamDescriptor* descriptors[] = {stream_descriptor.Get()};
    PC_RETURN_IF_FAILED(MFCreatePresentationDescriptor(1, descriptors, &descriptor_));
    PC_RETURN_IF_FAILED(MFCreateEventQueue(&queue_));
    return S_OK;
}

MediaStream* MediaSource::FindStream(DWORD id, DWORD* index) {
    for (DWORD i = 0; i < streams_.size(); ++i) {
        if (streams_[i]->Id() == id) {
            *index = i;
            return streams_[i].Get();
        }
    }
    return nullptr;
}

// IUnknown

STDMETHODIMP MediaSource::QueryInterface(REFIID riid, void** object) {
    if (object == nullptr) {
        return E_POINTER;
    }
    if (riid == __uuidof(IUnknown) || riid == __uuidof(IMFMediaEventGenerator) ||
        riid == __uuidof(IMFMediaSource) || riid == __uuidof(IMFMediaSourceEx)) {
        *object = static_cast<IMFMediaSourceEx*>(this);
    } else if (riid == __uuidof(IMFAttributes)) {
        *object = static_cast<IMFAttributes*>(this);
    } else if (riid == __uuidof(IMFGetService)) {
        *object = static_cast<IMFGetService*>(this);
    } else if (riid == __uuidof(IKsControl)) {
        *object = static_cast<IKsControl*>(this);
    } else if (riid == __uuidof(IMFSampleAllocatorControl)) {
        *object = static_cast<IMFSampleAllocatorControl*>(this);
    } else {
        *object = nullptr;
        return E_NOINTERFACE;
    }
    AddRef();
    return S_OK;
}

STDMETHODIMP_(ULONG) MediaSource::AddRef() { return references_.fetch_add(1) + 1; }

STDMETHODIMP_(ULONG) MediaSource::Release() {
    const ULONG remaining = references_.fetch_sub(1) - 1;
    if (remaining == 0) {
        delete this;
    }
    return remaining;
}

// IMFMediaEventGenerator

STDMETHODIMP MediaSource::BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) {
    ComPtr<IMFMediaEventQueue> queue;
    {
        ExclusiveGuard guard(lock_);
        queue = queue_;
    }
    return queue ? queue->BeginGetEvent(callback, state) : MF_E_SHUTDOWN;
}

STDMETHODIMP MediaSource::EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) {
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

STDMETHODIMP MediaSource::GetEvent(DWORD flags, IMFMediaEvent** event) {
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

STDMETHODIMP MediaSource::QueueEvent(MediaEventType type, REFGUID extended_type, HRESULT status,
                                     const PROPVARIANT* value) {
    ComPtr<IMFMediaEventQueue> queue;
    {
        ExclusiveGuard guard(lock_);
        queue = queue_;
    }
    return queue ? queue->QueueEventParamVar(type, extended_type, status, value) : MF_E_SHUTDOWN;
}

// IMFMediaSource

STDMETHODIMP MediaSource::CreatePresentationDescriptor(IMFPresentationDescriptor** descriptor) {
    if (descriptor == nullptr) {
        return E_POINTER;
    }
    *descriptor = nullptr;
    ExclusiveGuard guard(lock_);
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }
    return descriptor_->Clone(descriptor);
}

STDMETHODIMP MediaSource::GetCharacteristics(DWORD* characteristics) {
    if (characteristics == nullptr) {
        return E_POINTER;
    }
    ExclusiveGuard guard(lock_);
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }
    *characteristics = MFMEDIASOURCE_IS_LIVE;
    return S_OK;
}

STDMETHODIMP MediaSource::Pause() {
    // A live camera cannot pause; Frame Server pauses individual streams instead.
    return MF_E_INVALID_STATE_TRANSITION;
}

STDMETHODIMP MediaSource::Shutdown() {
    ComPtr<IMFMediaEventQueue> queue;
    std::vector<ComPtr<MediaStream>> streams;
    {
        ExclusiveGuard guard(lock_);
        if (shut_down_) {
            return MF_E_SHUTDOWN;
        }
        shut_down_ = true;
        queue.Swap(queue_);
        streams.swap(streams_);
        descriptor_.Reset();
    }
    // Streams join their delivery threads, so this runs outside the lock.
    for (auto& stream : streams) {
        stream->Shutdown();
    }
    if (queue) {
        queue->Shutdown();
    }
    return S_OK;
}

STDMETHODIMP MediaSource::Start(IMFPresentationDescriptor* descriptor, const GUID* time_format,
                                const PROPVARIANT* start_position) {
    if (descriptor == nullptr || start_position == nullptr) {
        return E_INVALIDARG;
    }
    if (time_format != nullptr && *time_format != GUID_NULL) {
        return MF_E_UNSUPPORTED_TIME_FORMAT;
    }

    ExclusiveGuard guard(lock_);
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }

    DWORD count = 0;
    PC_RETURN_IF_FAILED(descriptor->GetStreamDescriptorCount(&count));
    if (count != streams_.size()) {
        return E_INVALIDARG;
    }

    // Walk the caller's descriptor. Selected streams start (MENewStream, or MEUpdatedStream if
    // they were already running) with the media type the caller chose; deselected streams that
    // were running stop.
    for (DWORD i = 0; i < count; ++i) {
        BOOL selected = FALSE;
        ComPtr<IMFStreamDescriptor> stream_descriptor;
        PC_RETURN_IF_FAILED(descriptor->GetStreamDescriptorByIndex(i, &selected, &stream_descriptor));
        DWORD id = 0;
        PC_RETURN_IF_FAILED(stream_descriptor->GetStreamIdentifier(&id));

        DWORD index = 0;
        MediaStream* stream = FindStream(id, &index);
        if (stream == nullptr) {
            return MF_E_INVALIDSTREAMNUMBER;
        }
        const bool was_active = stream->IsActive();

        if (selected) {
            ComPtr<IMFMediaTypeHandler> handler;
            ComPtr<IMFMediaType> type;
            PC_RETURN_IF_FAILED(stream_descriptor->GetMediaTypeHandler(&handler));
            PC_RETURN_IF_FAILED(handler->GetCurrentMediaType(&type));

            PC_RETURN_IF_FAILED(descriptor_->SelectStream(index));
            PC_RETURN_IF_FAILED(queue_->QueueEventParamUnk(
                was_active ? MEUpdatedStream : MENewStream, GUID_NULL, S_OK,
                static_cast<IMFMediaStream*>(stream)));
            PC_RETURN_IF_FAILED(stream->Start(type.Get()));
        } else if (was_active) {
            PC_RETURN_IF_FAILED(descriptor_->DeselectStream(index));
            PC_RETURN_IF_FAILED(stream->Stop());
        }
    }

    PROPVARIANT time;
    PC_RETURN_IF_FAILED(InitPropVariantFromInt64(MFGetSystemTime(), &time));
    const HRESULT result = queue_->QueueEventParamVar(MESourceStarted, GUID_NULL, S_OK, &time);
    PropVariantClear(&time);
    return result;
}

STDMETHODIMP MediaSource::Stop() {
    ExclusiveGuard guard(lock_);
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }
    for (DWORD i = 0; i < streams_.size(); ++i) {
        if (streams_[i]->IsActive()) {
            PC_RETURN_IF_FAILED(streams_[i]->Stop());
        }
        PC_RETURN_IF_FAILED(descriptor_->DeselectStream(i));
    }

    PROPVARIANT time;
    PC_RETURN_IF_FAILED(InitPropVariantFromInt64(MFGetSystemTime(), &time));
    const HRESULT result = queue_->QueueEventParamVar(MESourceStopped, GUID_NULL, S_OK, &time);
    PropVariantClear(&time);
    return result;
}

// IMFMediaSourceEx

STDMETHODIMP MediaSource::GetSourceAttributes(IMFAttributes** attributes) {
    if (attributes == nullptr) {
        return E_POINTER;
    }
    *attributes = nullptr;
    ExclusiveGuard guard(lock_);
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }
    return attributes_.CopyTo(attributes);
}

STDMETHODIMP MediaSource::GetStreamAttributes(DWORD stream_id, IMFAttributes** attributes) {
    if (attributes == nullptr) {
        return E_POINTER;
    }
    *attributes = nullptr;
    ExclusiveGuard guard(lock_);
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }
    DWORD index = 0;
    MediaStream* stream = FindStream(stream_id, &index);
    if (stream == nullptr) {
        return MF_E_INVALIDSTREAMNUMBER;
    }
    return stream->QueryInterface(IID_PPV_ARGS(attributes));
}

STDMETHODIMP MediaSource::SetD3DManager(IUnknown*) {
    // PairCam produces system-memory samples on every path (see MediaStream::Start). Frame
    // Server ignores this return value; it only informs the source that a GPU is available.
    return E_NOTIMPL;
}

// IMFGetService

STDMETHODIMP MediaSource::GetService(REFGUID, REFIID, void** object) {
    if (object != nullptr) {
        *object = nullptr;
    }
    return MF_E_UNSUPPORTED_SERVICE;
}

// IMFSampleAllocatorControl

STDMETHODIMP MediaSource::SetDefaultAllocator(DWORD output_stream_id, IUnknown* allocator) {
    ExclusiveGuard guard(lock_);
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }
    DWORD index = 0;
    MediaStream* stream = FindStream(output_stream_id, &index);
    if (stream == nullptr) {
        return MF_E_INVALIDSTREAMNUMBER;
    }
    return stream->SetAllocator(allocator);
}

STDMETHODIMP MediaSource::GetAllocatorUsage(DWORD output_stream_id, DWORD* input_stream_id,
                                            MFSampleAllocatorUsage* usage) {
    if (input_stream_id == nullptr || usage == nullptr) {
        return E_POINTER;
    }
    ExclusiveGuard guard(lock_);
    if (shut_down_) {
        return MF_E_SHUTDOWN;
    }
    DWORD index = 0;
    if (FindStream(output_stream_id, &index) == nullptr) {
        return MF_E_INVALIDSTREAMNUMBER;
    }
    *input_stream_id = output_stream_id;
    *usage = MFSampleAllocatorUsage_UsesProvidedAllocator;
    return S_OK;
}

// IKsControl: no camera controls yet (see MediaStream).

STDMETHODIMP_(NTSTATUS) MediaSource::KsProperty(PKSPROPERTY, ULONG, void*, ULONG,
                                               ULONG* returned) {
    if (returned != nullptr) {
        *returned = 0;
    }
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

STDMETHODIMP_(NTSTATUS) MediaSource::KsMethod(PKSMETHOD, ULONG, void*, ULONG, ULONG* returned) {
    if (returned != nullptr) {
        *returned = 0;
    }
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

STDMETHODIMP_(NTSTATUS) MediaSource::KsEvent(PKSEVENT, ULONG, void*, ULONG, ULONG* returned) {
    if (returned != nullptr) {
        *returned = 0;
    }
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}
