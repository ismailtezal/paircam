// COM server entry points of paircam_vcam.dll. Registration follows smourier/VCamSample
// (dllmain.cpp, MIT; see the notice in media_source.cpp).

#include "common.h"

#include <cstdarg>
#include <cstdio>
#include <new>

#include "activator.h"

const CLSID CLSID_PairCamVirtualCamera = PCVC_CLSID_INIT;
std::atomic<long> g_objectCount{0};
std::atomic<long> g_lockCount{0};

namespace {

HMODULE g_module = nullptr;

constexpr wchar_t kClsidKey[] = L"Software\\Classes\\CLSID\\" PCVC_CLSID_STRING;

class ClassFactory final : public IClassFactory, private ModuleObject {
public:
    STDMETHODIMP QueryInterface(REFIID riid, void** object) override {
        if (object == nullptr) {
            return E_POINTER;
        }
        if (riid == __uuidof(IUnknown) || riid == __uuidof(IClassFactory)) {
            *object = static_cast<IClassFactory*>(this);
            AddRef();
            return S_OK;
        }
        *object = nullptr;
        return E_NOINTERFACE;
    }

    STDMETHODIMP_(ULONG) AddRef() override { return references_.fetch_add(1) + 1; }

    STDMETHODIMP_(ULONG) Release() override {
        const ULONG remaining = references_.fetch_sub(1) - 1;
        if (remaining == 0) {
            delete this;
        }
        return remaining;
    }

    STDMETHODIMP CreateInstance(IUnknown* outer, REFIID riid, void** object) override {
        if (object == nullptr) {
            return E_POINTER;
        }
        *object = nullptr;
        if (outer != nullptr) {
            return CLASS_E_NOAGGREGATION;
        }
        return Activator::Create(riid, object);
    }

    STDMETHODIMP LockServer(BOOL lock) override {
        if (lock) {
            g_lockCount.fetch_add(1);
        } else {
            g_lockCount.fetch_sub(1);
        }
        return S_OK;
    }

private:
    std::atomic<ULONG> references_{1};
};

LSTATUS SetStringValue(HKEY key, const wchar_t* name, const wchar_t* value) {
    const DWORD bytes = static_cast<DWORD>((wcslen(value) + 1) * sizeof(wchar_t));
    return RegSetValueExW(key, name, 0, REG_SZ, reinterpret_cast<const BYTE*>(value), bytes);
}

} // namespace

void TraceLine(const wchar_t* format, ...) {
    if (!IsDebuggerPresent()) {
        return;
    }
    wchar_t text[512];
    int prefix = swprintf_s(text, L"[PairCam vcam %lu] ", GetCurrentProcessId());
    if (prefix < 0) {
        prefix = 0;
    }
    va_list arguments;
    va_start(arguments, format);
    vswprintf_s(text + prefix, ARRAYSIZE(text) - prefix - 2, format, arguments);
    va_end(arguments);
    wcscat_s(text, L"\n");
    OutputDebugStringW(text);
}

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, void*) {
    if (reason == DLL_PROCESS_ATTACH) {
        g_module = module;
        DisableThreadLibraryCalls(module);
    }
    return TRUE;
}

_Check_return_ STDAPI DllGetClassObject(_In_ REFCLSID clsid, _In_ REFIID riid,
                                        _Outptr_ void** object) {
    if (object == nullptr) {
        return E_POINTER;
    }
    *object = nullptr;
    if (clsid != CLSID_PairCamVirtualCamera) {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    auto* factory = new (std::nothrow) ClassFactory();
    if (factory == nullptr) {
        return E_OUTOFMEMORY;
    }
    const HRESULT result = factory->QueryInterface(riid, object);
    factory->Release();
    return result;
}

__control_entrypoint(DllExport) STDAPI DllCanUnloadNow() {
    return g_objectCount.load() == 0 && g_lockCount.load() == 0 ? S_OK : S_FALSE;
}

// Frame Server only loads virtual camera sources registered machine-wide, so this writes HKLM
// and needs an elevated caller (regsvr32 from scripts/register-vcam.ps1).
STDAPI DllRegisterServer() {
    wchar_t path[MAX_PATH];
    const DWORD length = GetModuleFileNameW(g_module, path, ARRAYSIZE(path));
    if (length == 0 || length >= ARRAYSIZE(path)) {
        return HRESULT_FROM_WIN32(ERROR_BUFFER_OVERFLOW);
    }

    HKEY clsid = nullptr;
    LSTATUS status = RegCreateKeyExW(HKEY_LOCAL_MACHINE, kClsidKey, 0, nullptr,
                                     REG_OPTION_NON_VOLATILE, KEY_WRITE, nullptr, &clsid, nullptr);
    if (status != ERROR_SUCCESS) {
        return HRESULT_FROM_WIN32(status);
    }
    status = SetStringValue(clsid, nullptr, L"" PCVC_FRIENDLY_NAME);

    HKEY server = nullptr;
    if (status == ERROR_SUCCESS) {
        status = RegCreateKeyExW(clsid, L"InprocServer32", 0, nullptr, REG_OPTION_NON_VOLATILE,
                                 KEY_WRITE, nullptr, &server, nullptr);
    }
    if (status == ERROR_SUCCESS) {
        status = SetStringValue(server, nullptr, path);
    }
    if (status == ERROR_SUCCESS) {
        status = SetStringValue(server, L"ThreadingModel", L"Both");
    }
    if (server != nullptr) {
        RegCloseKey(server);
    }
    RegCloseKey(clsid);
    return HRESULT_FROM_WIN32(status);
}

STDAPI DllUnregisterServer() {
    const LSTATUS status = RegDeleteTreeW(HKEY_LOCAL_MACHINE, kClsidKey);
    if (status == ERROR_FILE_NOT_FOUND) {
        return S_OK;
    }
    return HRESULT_FROM_WIN32(status);
}
