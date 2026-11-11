import { colorScheme, useColorScheme } from 'nativewind';
import { cubicBezier, Easing, type WithSpringConfig } from 'react-native-reanimated';

import { tokens, type ColorMode, type ColorName } from './tokens';

export { tokens };
export type { ColorMode, ColorName };

/** What the user chose in Settings. Dark is the default; light is a full peer. */
export type Appearance = 'system' | 'dark' | 'light';

export const DEFAULT_APPEARANCE: Appearance = 'dark';

/** Switches every `className` colour and `useTheme()` reader to the chosen appearance. */
export function applyAppearance(appearance: Appearance) {
  colorScheme.set(appearance);
}

/**
 * The resolved colours for places a class name cannot reach: native props such as an icon tint,
 * the blur tint or the navigation header.
 */
export function useTheme() {
  const { colorScheme: scheme } = useColorScheme();
  const mode: ColorMode = scheme === 'light' ? 'light' : 'dark';
  return { mode, colors: tokens.color[mode] };
}

type SpringName = keyof typeof tokens.motion.spring;

const spring = (name: SpringName): WithSpringConfig => {
  const { mass, stiffness, damping } = tokens.motion.spring[name];
  return { mass, stiffness, damping };
};

/** Motion presets from the tokens, in the shapes Reanimated wants. */
export const motion = {
  duration: tokens.motion.duration,
  /** For `withSpring`. Reduced motion is honoured by Reanimated's default. */
  spring: {
    snappy: spring('snappy'),
    smooth: spring('smooth'),
    gentle: spring('gentle'),
  },
  /** For `withTiming`. */
  easing: {
    out: Easing.bezier(...tokens.motion.easing.out),
    inOut: Easing.bezier(...tokens.motion.easing.inOut),
    drawer: Easing.bezier(...tokens.motion.easing.drawer),
  },
  /** For CSS-style transitions on animated components. */
  transitionEasing: {
    out: cubicBezier(...tokens.motion.easing.out),
    inOut: cubicBezier(...tokens.motion.easing.inOut),
  },
} as const;

/**
 * The blur behind translucent controls. The tint and border of the material are colour tokens;
 * the blur radius is not in the token file yet and should move there (`elevation.material`).
 */
export const MATERIAL_BLUR_INTENSITY = 60;

/** How far a pressed control shrinks. Small enough to feel physical without being noticed. */
export const PRESSED_SCALE = 0.97;

/** Text tracking for a font size: the token is a fraction of the size, React Native wants points. */
export function tracking(size: number, amount: keyof typeof tokens.font.tracking) {
  return size * tokens.font.tracking[amount];
}
