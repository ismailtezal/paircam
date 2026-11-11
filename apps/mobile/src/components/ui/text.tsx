// Based on the Text component from React Native Reusables (MIT, Copyright (c) Founded Labs),
// restyled for PairCam: the system font, a short type scale and tracking from the design tokens.

import { Slot } from '@rn-primitives/slot';
import { cva, type VariantProps } from 'class-variance-authority';
import * as React from 'react';
import { Text as RNText, type TextStyle } from 'react-native';

import { cn } from '@/lib/utils';
import { tokens, tracking } from '@/theme';

const textVariants = cva('font-sans text-text', {
  variants: {
    variant: {
      /** The one large line on a screen. */
      display: 'text-xl font-semibold',
      title: 'text-lg font-semibold',
      headline: 'text-md font-semibold',
      body: 'text-base font-regular',
      label: 'text-sm font-medium',
      caption: 'text-xs font-regular',
      /** Live numbers: fps, Mbps, percentages. */
      mono: 'font-mono text-sm font-regular',
      /** The pairing code. */
      code: 'font-mono text-xxl font-medium',
    },
    tone: {
      default: '',
      muted: 'text-text-muted',
      subtle: 'text-text-subtle',
      accent: 'text-accent',
      live: 'text-live',
      warning: 'text-warning',
      danger: 'text-danger',
    },
  },
  defaultVariants: {
    variant: 'body',
    tone: 'default',
  },
});

type TextVariant = NonNullable<VariantProps<typeof textVariants>['variant']>;

const { size } = tokens.font;

// Large text is tracked tighter; small text is left alone. Numerals in mono text keep a fixed
// width so live values do not jitter as they change.
const VARIANT_STYLE: Record<TextVariant, TextStyle> = {
  display: { letterSpacing: tracking(size.xl, 'tight') },
  title: { letterSpacing: tracking(size.lg, 'tight') },
  headline: { letterSpacing: tracking(size.md, 'tight') },
  body: { letterSpacing: tracking(size.base, 'normal') },
  label: { letterSpacing: tracking(size.sm, 'normal') },
  caption: { letterSpacing: tracking(size.xs, 'normal') },
  mono: { fontVariant: ['tabular-nums'] },
  code: { fontVariant: ['tabular-nums'], letterSpacing: tracking(size.xxl, 'wide') },
};

/** Lets a container, such as a button, set the text colour of the labels inside it. */
const TextClassContext = React.createContext<string | undefined>(undefined);

type TextProps = React.ComponentProps<typeof RNText> &
  VariantProps<typeof textVariants> & {
    asChild?: boolean;
  };

function Text({ className, asChild = false, variant, tone, style, ...props }: TextProps) {
  const textClass = React.useContext(TextClassContext);
  const Component = asChild ? Slot : RNText;
  return (
    <Component
      className={cn(textVariants({ variant, tone }), textClass, className)}
      style={[VARIANT_STYLE[variant ?? 'body'], style]}
      {...props}
    />
  );
}

export { Text, TextClassContext, textVariants };
export type { TextProps };
