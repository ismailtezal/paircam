import Storage from 'expo-sqlite/kv-store';

// Small, synchronous settings storage. Synchronous reads matter at launch: the language and
// appearance must be known before the first frame, or the app would flash the wrong one.

const KEYS = {
  language: 'settings.language',
  appearance: 'settings.appearance',
  faceTracking: 'settings.faceTracking',
  gestures: 'settings.gestures',
  onboardingComplete: 'onboarding.complete',
} as const;

type Key = keyof typeof KEYS;

export function readSetting(key: Key): string | null {
  try {
    return Storage.getItemSync(KEYS[key]);
  } catch {
    return null;
  }
}

export function writeSetting(key: Key, value: string | null) {
  try {
    if (value === null) {
      Storage.removeItemSync(KEYS[key]);
    } else {
      Storage.setItemSync(KEYS[key], value);
    }
  } catch {
    // A setting that fails to save falls back to its default next launch; nothing to show.
  }
}
