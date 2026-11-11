import { useSyncExternalStore } from 'react';

import { engine } from '@/engine/store';
import { languagePreference, setLanguagePreference, type LanguagePreference } from '@/i18n';
import { readSetting, writeSetting } from '@/lib/storage';
import { applyAppearance, DEFAULT_APPEARANCE, type Appearance } from '@/theme';

// What the user can choose in Settings. Each is saved, applied at once, and readable from any
// screen with `usePreferences`.

export const APPEARANCES: readonly Appearance[] = ['system', 'dark', 'light'];

type Preferences = {
  /** `null` follows the iPhone's language. */
  language: LanguagePreference;
  appearance: Appearance;
  /** Face tracking starts on with every session. Off unless the user turns it on. */
  faceTracking: boolean;
  /** Hand gestures are watched for in every session. Off unless the user turns them on. */
  gestures: boolean;
};

function storedAppearance(): Appearance {
  const stored = readSetting('appearance') as Appearance | null;
  return stored && APPEARANCES.includes(stored) ? stored : DEFAULT_APPEARANCE;
}

let preferences: Preferences = {
  language: languagePreference(),
  appearance: storedAppearance(),
  faceTracking: readSetting('faceTracking') === 'true',
  gestures: readSetting('gestures') === 'true',
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

export function setFaceTracking(enabled: boolean) {
  writeSetting('faceTracking', String(enabled));
  engine.setFaceTracking(enabled);
  update({ faceTracking: enabled });
}

export function setGestures(enabled: boolean) {
  writeSetting('gestures', String(enabled));
  engine.setGestures(enabled);
  update({ gestures: enabled });
}

/** Applies the saved appearance. Call once, before the first render. */
export function applyStoredAppearance() {
  applyAppearance(preferences.appearance);
}

/** Hands the saved face tracking and gesture choices to the engine for the next session. */
export function applyStoredCameraControls() {
  engine.setFaceTracking(preferences.faceTracking);
  engine.setGestures(preferences.gestures);
}
