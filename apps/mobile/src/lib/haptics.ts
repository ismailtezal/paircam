import * as Haptics from 'expo-haptics';

// One haptic per meaningful action, fired at the moment the action happens. Haptics are never the
// only feedback: every one of these accompanies a visible change.

const ignore = () => {
  // Haptics are unavailable on some devices and in Low Power Mode; that is not an error.
};

export const haptics = {
  /** A control changed position: a toggle, a selection. */
  selection() {
    Haptics.selectionAsync().catch(ignore);
  },
  /** Something committed or snapped into place. */
  light() {
    Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light).catch(ignore);
  },
  /** A weighty or destructive action fired. */
  medium() {
    Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(ignore);
  },
  success() {
    Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success).catch(ignore);
  },
  /** Something needs the user's attention. */
  warning() {
    Haptics.notificationAsync(Haptics.NotificationFeedbackType.Warning).catch(ignore);
  },
  error() {
    Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error).catch(ignore);
  },
};

export type Haptic = keyof typeof haptics;
