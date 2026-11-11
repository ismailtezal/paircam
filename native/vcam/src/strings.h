// The camera DLL's string catalog.
//
// The DLL runs inside a Windows service, outside the app, so it cannot use the app's
// translation files. It shows exactly one sentence (on the placeholder picture), picked by the
// system's interface language because a service has no signed-in user's language. English is
// the fallback. Add a language by adding a row.

#pragma once

#include <windows.h>

struct PlaceholderStrings {
    LANGID primary_language;
    const wchar_t* hint;
};

inline constexpr PlaceholderStrings kPlaceholderStrings[] = {
    {LANG_ENGLISH, L"Open PairCam on your phone"},
    {LANG_TURKISH, L"Telefonunuzda PairCam’i açın"},
};

inline const wchar_t* PlaceholderHint() {
    const LANGID language = PRIMARYLANGID(GetUserDefaultUILanguage());
    for (const auto& entry : kPlaceholderStrings) {
        if (entry.primary_language == language) {
            return entry.hint;
        }
    }
    return kPlaceholderStrings[0].hint;
}

// The wordmark is the product name and is never translated.
inline constexpr wchar_t kWordmark[] = L"PairCam";
