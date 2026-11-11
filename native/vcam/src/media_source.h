// The PairCam camera as a Media Foundation media source, in the shape Frame Server requires of a
// virtual camera: IMFMediaSourceEx with source and stream attributes, IMFGetService, IKsControl
// and IMFSampleAllocatorControl.

#pragma once

#include "attributes.h"
#include "media_stream.h"

class MediaSource final : public IMFMediaSourceEx,
                          public AttributesImpl<IMFAttributes>,
                          public IMFGetService,
                          public IKsControl,
                          public IMFSampleAllocatorControl,
                          private ModuleObject {
public:
    // `activator_attributes` are copied into the source's attributes, as Frame Server expects.
    static HRESULT Create(IMFAttributes* activator_attributes, MediaSource** source);

    // IUnknown
    STDMETHODIMP QueryInterface(REFIID riid, void** object) override;
    STDMETHODIMP_(ULONG) AddRef() override;
    STDMETHODIMP_(ULONG) Release() override;

    // IMFMediaEventGenerator
    STDMETHODIMP BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) override;
    STDMETHODIMP EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) override;
    STDMETHODIMP GetEvent(DWORD flags, IMFMediaEvent** event) override;
    STDMETHODIMP QueueEvent(MediaEventType type, REFGUID extended_type, HRESULT status,
                            const PROPVARIANT* value) override;

    // IMFMediaSource
    STDMETHODIMP CreatePresentationDescriptor(IMFPresentationDescriptor** descriptor) override;
    STDMETHODIMP GetCharacteristics(DWORD* characteristics) override;
    STDMETHODIMP Pause() override;
    STDMETHODIMP Shutdown() override;
    STDMETHODIMP Start(IMFPresentationDescriptor* descriptor, const GUID* time_format,
                       const PROPVARIANT* start_position) override;
    STDMETHODIMP Stop() override;

    // IMFMediaSourceEx
    STDMETHODIMP GetSourceAttributes(IMFAttributes** attributes) override;
    STDMETHODIMP GetStreamAttributes(DWORD stream_id, IMFAttributes** attributes) override;
    STDMETHODIMP SetD3DManager(IUnknown* manager) override;

    // IMFGetService
    STDMETHODIMP GetService(REFGUID service, REFIID riid, void** object) override;

    // IMFSampleAllocatorControl
    STDMETHODIMP SetDefaultAllocator(DWORD output_stream_id, IUnknown* allocator) override;
    STDMETHODIMP GetAllocatorUsage(DWORD output_stream_id, DWORD* input_stream_id,
                                   MFSampleAllocatorUsage* usage) override;

    // IKsControl
    STDMETHODIMP_(NTSTATUS) KsProperty(PKSPROPERTY property, ULONG property_length, void* data,
                                       ULONG data_length, ULONG* returned) override;
    STDMETHODIMP_(NTSTATUS) KsMethod(PKSMETHOD method, ULONG method_length, void* data,
                                     ULONG data_length, ULONG* returned) override;
    STDMETHODIMP_(NTSTATUS) KsEvent(PKSEVENT event, ULONG event_length, void* data,
                                    ULONG data_length, ULONG* returned) override;

private:
    MediaSource() = default;
    ~MediaSource() = default;

    HRESULT Initialize(IMFAttributes* activator_attributes);
    // The stream with `id` and its index, or null. Requires lock_.
    MediaStream* FindStream(DWORD id, DWORD* index);

    std::atomic<ULONG> references_{1};

    // Guards everything below. Event queue calls happen outside it (see MediaStream).
    SrwLock lock_;
    bool shut_down_ = false;
    ComPtr<IMFMediaEventQueue> queue_;
    ComPtr<IMFPresentationDescriptor> descriptor_;
    std::vector<ComPtr<MediaStream>> streams_;
};
