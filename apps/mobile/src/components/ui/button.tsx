// Based on the Button component from React Native Reusables (MIT, Copyright (c) Founded Labs),
// restyled for PairCam: near-monochrome, hairline borders, continuous corners, a 44 pt minimum
// height, and a press that shrinks instead of fading.

import { cva, type VariantProps } from 'class-variance-authority';
import * as React from 'react';
import { ActivityIndicator, StyleSheet, View } from 'react-native';

import { cn } from '@/lib/utils';
import { useTheme, type ColorName } from '@/theme';

import { PressableScale, type PressableScaleProps } from './pressable-scale';
import { Text, TextClassContext } from './text';

const buttonVariants = cva('flex-row items-center justify-center gap-2 border-hairline', {
  variants: {
    variant: {
      /** The one main action on a screen. */
      primary: 'border-primary bg-primary',
      /** A quiet alternative next to a primary action. */
      secondary: 'border-border-strong bg-surface-raised',
      /** A secondary action floating over the camera preview. */
      material: 'border-material-border bg-material',
      /** Ends something: stop the stream, forget a computer. */
      danger: 'border-danger bg-danger',
      ghost: 'border-transparent bg-transparent',
    },
    size: {
      md: 'h-touch-target rounded-lg px-4',
      lg: 'h-12 rounded-lg px-5',
    },
    pressed: {
      true: '',
      false: '',
    },
  },
  compoundVariants: [
    { variant: 'primary', pressed: true, className: 'border-primary-active bg-primary-active' },
    { variant: 'secondary', pressed: true, className: 'bg-surface-active' },
    { variant: 'material', pressed: true, className: 'bg-surface-active' },
    { variant: 'ghost', pressed: true, className: 'bg-surface-hover' },
  ],
  defaultVariants: {
    variant: 'primary',
    size: 'lg',
    pressed: false,
  },
});

const buttonTextVariants = cva('text-md font-semibold', {
  variants: {
    variant: {
      primary: 'text-primary-foreground',
      secondary: 'text-text',
      material: 'text-text',
      danger: 'text-danger-foreground',
      ghost: 'text-text',
    },
  },
  defaultVariants: {
    variant: 'primary',
  },
});

type ButtonVariant = NonNullable<VariantProps<typeof buttonVariants>['variant']>;

const SPINNER_COLOR: Record<ButtonVariant, ColorName> = {
  primary: 'primaryForeground',
  secondary: 'text',
  material: 'text',
  danger: 'dangerForeground',
  ghost: 'text',
};

type ButtonProps = Omit<PressableScaleProps, 'children'> &
  Omit<VariantProps<typeof buttonVariants>, 'pressed'> & {
    /** The label. Pass a translated string. */
    label: string;
    /** Shows a spinner in place of the label and ignores presses. */
    loading?: boolean;
    /** Something to show before the label, usually an icon. */
    leading?: React.ReactNode;
    className?: string;
  };

function Button({
  label,
  loading = false,
  leading,
  variant,
  size,
  className,
  disabled,
  ...props
}: ButtonProps) {
  const { colors } = useTheme();
  const resolvedVariant = variant ?? 'primary';
  const inactive = disabled === true || loading;

  return (
    <TextClassContext.Provider value={buttonTextVariants({ variant: resolvedVariant })}>
      <PressableScale
        accessibilityRole="button"
        accessibilityLabel={label}
        accessibilityState={{ disabled: inactive, busy: loading }}
        disabled={inactive}
        {...props}
      >
        {(pressed) => (
          <View
            className={cn(
              buttonVariants({ variant: resolvedVariant, size, pressed }),
              disabled === true && 'opacity-50',
              className,
            )}
            style={styles.continuous}
          >
            {loading ? (
              <ActivityIndicator color={colors[SPINNER_COLOR[resolvedVariant]]} />
            ) : (
              <>
                {leading}
                <Text numberOfLines={1}>{label}</Text>
              </>
            )}
          </View>
        )}
      </PressableScale>
    </TextClassContext.Provider>
  );
}

const styles = StyleSheet.create({
  continuous: {
    borderCurve: 'continuous',
  },
});

export { Button, buttonTextVariants, buttonVariants };
export type { ButtonProps };
