// The object Frame Server creates from the CLSID. It carries the virtual camera's attributes and
// creates a media source on ActivateObject.

#pragma once

#include "attributes.h"
#include "media_source.h"

class Activator final : public AttributesImpl<IMFActivate>, private ModuleObject {
public:
    static HRESULT Create(REFIID riid, void** object);

    // IUnknown
    STDMETHODIMP QueryInterface(REFIID riid, void** object) override;
    STDMETHODIMP_(ULONG) AddRef() override;
    STDMETHODIMP_(ULONG) Release() override;

    // IMFActivate
    STDMETHODIMP ActivateObject(REFIID riid, void** object) override;
    STDMETHODIMP ShutdownObject() override;
    STDMETHODIMP DetachObject() override;

private:
    Activator() = default;
    ~Activator() = default;

    HRESULT Initialize();

    std::atomic<ULONG> references_{1};
    SrwLock lock_;
    ComPtr<MediaSource> source_;
};
