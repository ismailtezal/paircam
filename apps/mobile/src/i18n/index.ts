import { getLocales } from 'expo-localization';
import { createInstance } from 'i18next';
import { initReactI18next } from 'react-i18next';

import { readSetting, writeSetting } from '@/lib/storage';

import en from './locales/en.json';
import tr from './locales/tr.json';

// English is the source and the fallback. Adding a language means adding one catalog file with
// the same keys and listing it here; no component changes.

export const LANGUAGES = ['en', 'tr'] as const;
export type Language = (typeof LANGUAGES)[number];

export const FALLBACK_LANGUAGE: Language = 'en';

/** `null` means "follow the iPhone's language". */
export type LanguagePreference = Language | null;

const i18n = createInstance();

type Catalog = typeof en;

// Every catalog must have exactly the keys of the English one. These two assignments fail to
// compile when a key is missing from, or extra in, a translation.
const translations: Record<Language, Catalog> = { en, tr };
const sourceMatchesTranslations: typeof tr = en;
void sourceMatchesTranslations;

declare module 'i18next' {
  interface CustomTypeOptions {
    defaultNS: 'translation';
    resources: { translation: Catalog };
  }
}

function isLanguage(value: string | null | undefined): value is Language {
  return LANGUAGES.includes(value as Language);
}

/** The first of the user's preferred iPhone languages that PairCam is translated into. */
export function deviceLanguage(): Language {
  for (const locale of getLocales()) {
    if (isLanguage(locale.languageCode)) {
      return locale.languageCode;
    }
  }
  return FALLBACK_LANGUAGE;
}

export function languagePreference(): LanguagePreference {
  const stored = readSetting('language');
  return isLanguage(stored) ? stored : null;
}

export function setLanguagePreference(preference: LanguagePreference) {
  writeSetting('language', preference);
  void i18n.changeLanguage(preference ?? deviceLanguage());
}

void i18n.use(initReactI18next).init({
  resources: Object.fromEntries(
    LANGUAGES.map((language) => [language, { translation: translations[language] }]),
  ),
  lng: languagePreference() ?? deviceLanguage(),
  fallbackLng: FALLBACK_LANGUAGE,
  // The catalogs are bundled, so initialise in place: the first render is already translated.
  initAsync: false,
  interpolation: {
    // React already escapes text.
    escapeValue: false,
  },
});

export default i18n;
