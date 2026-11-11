#include "placeholder.h"

#include "common.h"
#include "convert.h"
#include "strings.h"

#include <d2d1.h>
#include <dwrite.h>
#include <wincodec.h>

namespace {

// Mirrors the dark theme in design/tokens/tokens.json: surface, text and textMuted. The DLL
// cannot read the generated theme at run time, so these three values are copied here.
constexpr uint32_t kBackground = 0x0A0A0A;
constexpr uint32_t kText = 0xEDEDED;
constexpr uint32_t kTextMuted = 0xA1A1A1;

constexpr float kWordmarkSize = 88.0f;
constexpr float kHintSize = 34.0f;
constexpr float kGap = 18.0f;

PlaceholderPicture g_picture;
INIT_ONCE g_once = INIT_ONCE_STATIC_INIT;

void FillPlain(std::vector<uint8_t>& bgra) {
    for (size_t offset = 0; offset < bgra.size(); offset += 4) {
        bgra[offset + 0] = static_cast<uint8_t>(kBackground & 0xFF);
        bgra[offset + 1] = static_cast<uint8_t>((kBackground >> 8) & 0xFF);
        bgra[offset + 2] = static_cast<uint8_t>((kBackground >> 16) & 0xFF);
        bgra[offset + 3] = 0xFF;
    }
}

HRESULT DrawCentredLine(ID2D1RenderTarget* target, IDWriteFactory* write, const wchar_t* text,
                        float size, DWRITE_FONT_WEIGHT weight, uint32_t color, float top) {
    ComPtr<IDWriteTextFormat> format;
    PC_RETURN_IF_FAILED(write->CreateTextFormat(L"Segoe UI Variable Display", nullptr, weight,
                                                DWRITE_FONT_STYLE_NORMAL,
                                                DWRITE_FONT_STRETCH_NORMAL, size, L"",
                                                &format));
    PC_RETURN_IF_FAILED(format->SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER));
    PC_RETURN_IF_FAILED(format->SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR));
    PC_RETURN_IF_FAILED(format->SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP));

    ComPtr<ID2D1SolidColorBrush> brush;
    PC_RETURN_IF_FAILED(target->CreateSolidColorBrush(D2D1::ColorF(color), &brush));

    const D2D1_RECT_F box = D2D1::RectF(0.0f, top, static_cast<float>(PCVC_WIDTH),
                                        top + size * 1.6f);
    target->DrawText(text, static_cast<UINT32>(wcslen(text)), format.Get(), box, brush.Get());
    return S_OK;
}

HRESULT RenderText(std::vector<uint8_t>& bgra) {
    ComPtr<IWICImagingFactory> imaging;
    PC_RETURN_IF_FAILED(CoCreateInstance(CLSID_WICImagingFactory, nullptr,
                                         CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&imaging)));
    ComPtr<IWICBitmap> bitmap;
    PC_RETURN_IF_FAILED(imaging->CreateBitmap(PCVC_WIDTH, PCVC_HEIGHT,
                                              GUID_WICPixelFormat32bppPBGRA,
                                              WICBitmapCacheOnLoad, &bitmap));

    ComPtr<ID2D1Factory> d2d;
    PC_RETURN_IF_FAILED(D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, d2d.GetAddressOf()));
    // Software rendering: the Frame Server may have no usable GPU session, and this runs once.
    const D2D1_RENDER_TARGET_PROPERTIES properties = D2D1::RenderTargetProperties(
        D2D1_RENDER_TARGET_TYPE_SOFTWARE,
        D2D1::PixelFormat(DXGI_FORMAT_B8G8R8A8_UNORM, D2D1_ALPHA_MODE_PREMULTIPLIED), 96.0f,
        96.0f);
    ComPtr<ID2D1RenderTarget> target;
    PC_RETURN_IF_FAILED(d2d->CreateWicBitmapRenderTarget(bitmap.Get(), properties, &target));

    ComPtr<IDWriteFactory> write;
    PC_RETURN_IF_FAILED(DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED, __uuidof(IDWriteFactory),
                                            reinterpret_cast<IUnknown**>(write.GetAddressOf())));

    // Grayscale anti-aliasing: ClearType fringes would turn into colour noise once the picture
    // is subsampled to NV12 and encoded by the calling app.
    target->SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    target->BeginDraw();
    target->Clear(D2D1::ColorF(kBackground));

    const float block = kWordmarkSize * 1.3f + kGap + kHintSize * 1.3f;
    const float top = (static_cast<float>(PCVC_HEIGHT) - block) / 2.0f;
    HRESULT result = DrawCentredLine(target.Get(), write.Get(), kWordmark, kWordmarkSize,
                                     DWRITE_FONT_WEIGHT_SEMI_BOLD, kText, top);
    if (SUCCEEDED(result)) {
        result = DrawCentredLine(target.Get(), write.Get(), PlaceholderHint(), kHintSize,
                                 DWRITE_FONT_WEIGHT_NORMAL, kTextMuted,
                                 top + kWordmarkSize * 1.3f + kGap);
    }
    const HRESULT end = target->EndDraw();
    PC_RETURN_IF_FAILED(result);
    PC_RETURN_IF_FAILED(end);

    PC_RETURN_IF_FAILED(bitmap->CopyPixels(nullptr, PCVC_WIDTH * 4,
                                           static_cast<UINT>(bgra.size()), bgra.data()));
    for (size_t offset = 3; offset < bgra.size(); offset += 4) {
        bgra[offset] = 0xFF;
    }
    return S_OK;
}

DWORD WINAPI RenderThread(void* context) {
    auto* bgra = static_cast<std::vector<uint8_t>*>(context);
    const HRESULT com = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    HRESULT result = E_FAIL;
    if (SUCCEEDED(com)) {
        result = RenderText(*bgra);
        CoUninitialize();
    }
    return SUCCEEDED(result) ? 0 : 1;
}

BOOL CALLBACK RenderOnce(INIT_ONCE*, void*, void**) {
    PlaceholderPicture& picture = g_picture;
    picture.bgra.assign(static_cast<size_t>(PCVC_WIDTH) * PCVC_HEIGHT * 4, 0);
    FillPlain(picture.bgra);

    // A thread of our own gives DirectWrite and WIC a clean multithreaded apartment whatever the
    // caller's apartment is. If anything fails or hangs, the plain frame stays.
    auto* rendered = new (std::nothrow) std::vector<uint8_t>();
    if (rendered != nullptr) {
        rendered->assign(picture.bgra.size(), 0);
        UniqueHandle thread(CreateThread(nullptr, 0, RenderThread, rendered, 0, nullptr));
        bool finished = !thread.valid();
        if (thread.valid() && WaitForSingleObject(thread.get(), 10000) == WAIT_OBJECT_0) {
            finished = true;
            DWORD exit_code = 1;
            if (GetExitCodeThread(thread.get(), &exit_code) && exit_code == 0) {
                picture.bgra.swap(*rendered);
                picture.has_text = true;
            }
        }
        if (finished) {
            delete rendered;
        } else {
            // The thread still writes into `rendered`; leak it rather than free memory in use.
            // This only happens if DirectWrite hangs, which would be a system fault.
            TraceLine(L"placeholder rendering timed out; using the plain frame");
        }
    }

    picture.nv12.assign(PCVC_NV12_SIZE, 0);
    Bgra32ToNv12(picture.bgra.data(), PCVC_WIDTH, PCVC_HEIGHT, picture.nv12.data());
    return TRUE;
}

} // namespace

const PlaceholderPicture& Placeholder() {
    InitOnceExecuteOnce(&g_once, RenderOnce, nullptr, nullptr);
    return g_picture;
}
