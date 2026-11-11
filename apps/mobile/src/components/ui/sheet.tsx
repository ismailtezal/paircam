import * as React from 'react';
import { StyleSheet, View } from 'react-native';
import Animated, { FadeIn, FadeOut, SlideInDown, SlideOutDown } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { motion, tokens } from '@/theme';

import { Material } from './material';

/** Sheets stay phone-sized in landscape instead of stretching across the screen. */
export const SHEET_MAX_WIDTH = tokens.space[16] * 7;

type SheetProps = {
  children: React.ReactNode;
  /** Read by VoiceOver when the sheet appears. */
  accessibilityLabel: string;
};

/**
 * A sheet that lives inside the camera screen, for decisions the engine asks for rather than
 * places the user navigates to. Render it while it should be open and stop rendering it to close
 * it; it animates both ways by itself.
 *
 * It rises from the bottom edge and leaves the same way. Nothing dragged it, so the spring
 * settles without overshoot, and leaving is quicker than arriving. With Reduce Motion on,
 * Reanimated skips the movement. The dimmed backdrop blocks the controls behind it: the sheet has
 * to be answered.
 */
function Sheet({ children, accessibilityLabel }: SheetProps) {
  const insets = useSafeAreaInsets();
  const spring = motion.spring.smooth;

  return (
    <View
      style={StyleSheet.absoluteFill}
      accessibilityViewIsModal
      accessibilityLabel={accessibilityLabel}
    >
      <Animated.View
        style={StyleSheet.absoluteFill}
        entering={FadeIn.duration(motion.duration.base)}
        exiting={FadeOut.duration(motion.duration.fast)}
      >
        <View className="flex-1 bg-overlay" />
      </Animated.View>

      <Animated.View
        style={[
          styles.dock,
          {
            paddingBottom: Math.max(insets.bottom, tokens.space[3]),
            paddingLeft: Math.max(insets.left, tokens.space[3]),
            paddingRight: Math.max(insets.right, tokens.space[3]),
          },
        ]}
        pointerEvents="box-none"
        entering={SlideInDown.springify()
          .mass(spring.mass ?? 1)
          .stiffness(spring.stiffness ?? 1)
          .damping(spring.damping ?? 1)}
        exiting={SlideOutDown.duration(motion.duration.base).easing(motion.easing.drawer)}
      >
        <Material className="w-full self-center p-6" style={styles.sheet}>
          {children}
        </Material>
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  dock: {
    position: 'absolute',
    left: 0,
    right: 0,
    bottom: 0,
  },
  sheet: {
    maxWidth: SHEET_MAX_WIDTH,
  },
});

export { Sheet };
export type { SheetProps };
