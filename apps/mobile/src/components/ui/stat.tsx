import * as React from 'react';
import { View } from 'react-native';

import { cn } from '@/lib/utils';

import { Text, type TextProps } from './text';

type StatProps = {
  label: string;
  value: string;
  /** Colours the value when it needs attention, such as a hot phone. */
  tone?: TextProps['tone'];
  className?: string;
};

/**
 * One live number with its name. Values are set in mono with fixed-width numerals so the layout
 * holds still while they change every second.
 */
function Stat({ label, value, tone, className }: StatProps) {
  return (
    <View className={cn('gap-0.5', className)} accessible accessibilityLabel={`${label}, ${value}`}>
      <Text variant="caption" tone="muted" numberOfLines={1}>
        {label}
      </Text>
      <Text variant="mono" tone={tone} numberOfLines={1}>
        {value}
      </Text>
    </View>
  );
}

export { Stat };
export type { StatProps };
