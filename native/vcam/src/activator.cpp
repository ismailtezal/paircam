// Adapted from smourier/VCamSample (Activator.cpp) and Microsoft's Windows-Camera VirtualCamera
// sample, both MIT. See the notice in media_source.cpp.

#include "activator.h"

HRESULT Activator::Create(REFIID riid, void** object) {
    *object = nullptr;
    auto* created = new (std::nothrow) Activator();
    if (created == nullptr) {
        return E_OUTOFMEMORY;
    }
    HRESULT result = created->Initialize();
    if (SUCCEEDED(result)) {
        result = created->QueryInterface(riid, object);
    }
    created->Release();
    return result;
}

HRESULT Activator::Initialize() {
    PC_RETURN_IF_FAILED(InitializeAttributes());
    // Tells Frame Server this source does not wrap a physical camera, and names our class so
    // it can recreate the activator. Same pair VCamSample sets.
    PC_RETURN_IF_FAILED(SetUINT32(MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES, 1));
    PC_RETURN_IF_FAILED(SetGUID(MFT_TRANSFORM_CLSID_Attribute, CLSID_PairCamVirtualCamera));
    return S_OK;
}

STDMETHODIMP Activator::QueryInterface(REFIID riid, void** object) {
    if (object == nullptr) {
        return E_POINTER;
    }
    if (riid == __uuidof(IUnknown) || riid == __uuidof(IMFAttributes) ||
        riid == __uuidof(IMFActivate)) {
        *object = static_cast<IMFActivate*>(this);
    } else {
        *object = nullptr;
        return E_NOINTERFACE;
    }
    AddRef();
    return S_OK;
}

STDMETHODIMP_(ULONG) Activator::AddRef() { return references_.fetch_add(1) + 1; }

STDMETHODIMP_(ULONG) Activator::Release() {
    const ULONG remaining = references_.fetch_sub(1) - 1;
    if (remaining == 0) {
        delete this;
    }
    return remaining;
}

STDMETHODIMP Activator::ActivateObject(REFIID riid, void** object) {
    if (object == nullptr) {
        return E_POINTER;
    }
    *object = nullptr;

    // Like Microsoft's sample, each activation gets a fresh source: a source that has been
    // shut down can never start again, and Frame Server activates again after a shutdown.
    // The attributes are copied at this point, so anything Frame Server set on the activator
    // (for example the client process id) reaches the source.
    ComPtr<MediaSource> source;
    PC_RETURN_IF_FAILED(MediaSource::Create(attributes_.Get(), &source));
    const HRESULT result = source->QueryInterface(riid, object);
    if (FAILED(result)) {
        source->Shutdown();
        return result;
    }

    ExclusiveGuard guard(lock_);
    source_ = source;
    return S_OK;
}

STDMETHODIMP Activator::ShutdownObject() {
    // Frame Server shuts the source down itself; shutting it down here as well would make it
    // see MF_E_SHUTDOWN from a source it still owns (VCamSample and Microsoft's sample also
    // leave this empty).
    return S_OK;
}

STDMETHODIMP Activator::DetachObject() {
    ExclusiveGuard guard(lock_);
    source_.Reset();
    return S_OK;
}
