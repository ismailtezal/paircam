import { BlurView } from 'expo-blur';
import * as React from 'react';
import { StyleSheet, View, type ViewProps } from 'react-native';

import { cn } from '@/lib/utils';
import { MATERIAL_BLUR_INTENSITY, useTheme } from '@/theme';

type MaterialProps = ViewProps & {
  className?: string;
};

/**
 * A translucent, blurred surface for controls that float over the camera preview, so the picture
 * stays visible beneath them. The blur is the system's, which turns solid by itself when the user
 * has Reduce Transparency on. A hairline border catches the edge instead of a shadow.
 *
 * Never put one material on top of another: legibility collapses.
 */
function Material({ className, style, children, ...props }: MaterialProps) {
  const { mode } = useTheme();
  return (
    <View
      className={cn('overflow-hidden rounded-xl border-hairline border-material-border', className)}
      style={[styles.continuous, style]}
      {...props}
    >
      <BlurView
        intensity={MATERIAL_BLUR_INTENSITY}
        tint={mode === 'dark' ? 'dark' : 'light'}
        style={StyleSheet.absoluteFill}
      />
      <View className="bg-material" style={StyleSheet.absoluteFill} pointerEvents="none" />
      {children}
    </View>
  );
}

const styles = StyleSheet.create({
  continuous: {
    borderCurve: 'continuous',
  },
});

export { Material };
export type { MaterialProps };
