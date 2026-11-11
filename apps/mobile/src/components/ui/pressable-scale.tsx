import * as React from 'react';
import { Pressable, type PressableProps, type StyleProp, type ViewStyle } from 'react-native';
import Animated from 'react-native-reanimated';

import { haptics, type Haptic } from '@/lib/haptics';
import { motion, PRESSED_SCALE, tokens } from '@/theme';

type PressableScaleProps = Omit<PressableProps, 'children' | 'style'> & {
  /** Receives whether the control is pressed, for colour changes that accompany the scale. */
  children: React.ReactNode | ((pressed: boolean) => React.ReactNode);
  /** Played when the press commits. */
  haptic?: Haptic;
  style?: StyleProp<ViewStyle>;
};

/**
 * The press behaviour every control shares: it responds the moment a finger lands (a slight
 * shrink that takes the label with it), commits on release, forgives a finger that drifts, and
 * always offers a 44 pt target.
 *
 * The shrink is a transition on the UI thread, so it stays smooth while JavaScript is busy and
 * retargets cleanly when a press is cancelled halfway.
 */
function PressableScale({
  children,
  haptic,
  onPress,
  onPressIn,
  onPressOut,
  style,
  ...props
}: PressableScaleProps) {
  const [pressed, setPressed] = React.useState(false);

  return (
    <Pressable
      hitSlop={tokens.space[2]}
      pressRetentionOffset={tokens.space[4]}
      {...props}
      style={style}
      onPressIn={(event) => {
        setPressed(true);
        onPressIn?.(event);
      }}
      onPressOut={(event) => {
        setPressed(false);
        onPressOut?.(event);
      }}
      onPress={(event) => {
        if (haptic) {
          haptics[haptic]();
        }
        onPress?.(event);
      }}
    >
      <Animated.View style={pressed && !props.disabled ? PRESSED_STYLE : REST_STYLE}>
        {typeof children === 'function' ? children(pressed) : children}
      </Animated.View>
    </Pressable>
  );
}

// Plain objects rather than StyleSheet.create: the transition properties are Reanimated's.
const TRANSITION = {
  transitionProperty: 'transform',
  transitionDuration: motion.duration.fast,
  transitionTimingFunction: motion.transitionEasing.out,
} as const;

const REST_STYLE = {
  ...TRANSITION,
  transform: [{ scale: 1 }],
};

const PRESSED_STYLE = {
  ...TRANSITION,
  transform: [{ scale: PRESSED_SCALE }],
};

export { PressableScale };
export type { PressableScaleProps };
