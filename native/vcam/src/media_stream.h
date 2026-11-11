// The camera's single video stream.
//
// Samples are produced on a delivery thread of the stream's own at a steady PCVC_FPS: each tick
// answers the oldest outstanding RequestSample with the newest phone frame (or the placeholder)
// and a timestamp on a regular grid. RequestSample itself only queues the request, so Frame
// Server's threads never wait for the pipe or for a conversion.

#pragma once

#include <deque>

#include "attributes.h"

class MediaStream final : public IMFMediaStream2,
                          public AttributesImpl<IMFAttributes>,
                          public IKsControl,
                          private ModuleObject {
public:
    static HRESULT Create(IMFMediaSource* source, DWORD id, MediaStream** stream);

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

    // IMFMediaStream
    STDMETHODIMP GetMediaSource(IMFMediaSource** source) override;
    STDMETHODIMP GetStreamDescriptor(IMFStreamDescriptor** descriptor) override;
    STDMETHODIMP RequestSample(IUnknown* token) override;

    // IMFMediaStream2
    STDMETHODIMP SetStreamState(MF_STREAM_STATE state) override;
    STDMETHODIMP GetStreamState(MF_STREAM_STATE* state) override;

    // IKsControl
    STDMETHODIMP_(NTSTATUS) KsProperty(PKSPROPERTY property, ULONG property_length, void* data,
                                       ULONG data_length, ULONG* returned) override;
    STDMETHODIMP_(NTSTATUS) KsMethod(PKSMETHOD method, ULONG method_length, void* data,
                                     ULONG data_length, ULONG* returned) override;
    STDMETHODIMP_(NTSTATUS) KsEvent(PKSEVENT event, ULONG event_length, void* data,
                                    ULONG data_length, ULONG* returned) override;

    // Called by the media source.
    DWORD Id() const { return id_; }
    bool IsActive();
    HRESULT Start(IMFMediaType* type);
    HRESULT Stop();
    void Shutdown();
    HRESULT SetAllocator(IUnknown* allocator);

private:
    MediaStream() = default;
    ~MediaStream();

    HRESULT Initialize(IMFMediaSource* source, DWORD id);
    void StartDelivery();
    void StopDelivery();
    static DWORD WINAPI DeliveryMain(void* context);
    void DeliveryLoop();
    void DeliverOne(LONGLONG time);
    HRESULT FillSample(IMFSample* sample, const GUID& format);

    std::atomic<ULONG> references_{1};
    DWORD id_ = 0;

    // Guards everything below. Never held while waiting for the delivery thread or for the
    // event queue, so no Frame Server call can block on it for long.
    SrwLock lock_;
    bool shut_down_ = false;
    MF_STREAM_STATE state_ = MF_STREAM_STATE_STOPPED;
    GUID format_ = GUID_NULL;
    ComPtr<IMFMediaSource> source_;
    ComPtr<IMFMediaEventQueue> queue_;
    ComPtr<IMFStreamDescriptor> descriptor_;
    // Frame Server provides an allocator through IMFSampleAllocatorControl. Without one (as in
    // the test harness), the stream creates its own system-memory allocator.
    ComPtr<IMFVideoSampleAllocatorEx> allocator_;
    // Outstanding RequestSample tokens, oldest first. A null token is a valid request too.
    std::deque<ComPtr<IUnknown>> tokens_;

    // Serialises Start, Stop, Shutdown and SetStreamState, which may join the delivery thread.
    // The delivery thread never takes it, so joining under it cannot deadlock.
    SrwLock control_lock_;
    UniqueHandle delivery_thread_;
    UniqueHandle delivery_stop_;
    std::atomic<bool> consumer_{false};
};
