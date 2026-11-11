import { useSyncExternalStore } from 'react';

import { languagePreference, setLanguagePreference, type LanguagePreference } from '@/i18n';
import { readSetting, writeSetting } from '@/lib/storage';
import { applyAppearance, DEFAULT_APPEARANCE, type Appearance } from '@/theme';

// The two things the user can choose in Settings. Each is saved, applied at once, and readable
// from any screen with `usePreferences`.

export const APPEARANCES: readonly Appearance[] = ['system', 'dark', 'light'];

type Preferences = {
  /** `null` follows the iPhone's language. */
  language: LanguagePreference;
  appearance: Appearance;
};

function storedAppearance(): Appearance {
  const stored = readSetting('appearance') as Appearance | null;
  return stored && APPEARANCES.includes(stored) ? stored : DEFAULT_APPEARANCE;
}

let preferences: Preferences = {
  language: languagePreference(),
  appearance: storedAppearance(),
};

const listeners = new Set<() => void>();

function update(change: Partial<Preferences>) {
  preferences = { ...preferences, ...change };
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function usePreferences(): Preferences {
  return useSyncExternalStore(subscribe, () => preferences);
}

export function setLanguage(language: LanguagePreference) {
  setLanguagePreference(language);
  update({ language });
}

export function setAppearance(appearance: Appearance) {
  writeSetting('appearance', appearance);
  applyAppearance(appearance);
  update({ appearance });
}

/** Applies the saved appearance. Call once, before the first render. */
export function applyStoredAppearance() {
  applyAppearance(preferences.appearance);
}
