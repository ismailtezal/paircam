import * as React from 'react';
import { View } from 'react-native';

import { cn } from '@/lib/utils';

import { Material } from './material';
import { Text } from './text';

type StatusTone = 'idle' | 'active' | 'live' | 'warning' | 'danger';

const DOT_CLASS: Record<StatusTone, string> = {
  idle: 'bg-text-subtle',
  active: 'bg-text',
  live: 'bg-live',
  warning: 'bg-warning',
  danger: 'bg-danger',
};

type StatusPillProps = {
  tone: StatusTone;
  label: string;
  className?: string;
};

/**
 * The connection state, readable at a glance: a coloured dot and a few words in a capsule that
 * floats over the preview. Colour here always means status.
 */
function StatusPill({ tone, label, className }: StatusPillProps) {
  return (
    <Material
      className={cn('h-touch-target flex-row items-center gap-2 rounded-full px-4', className)}
      accessible
      accessibilityRole="text"
      accessibilityLabel={label}
    >
      <View className={cn('size-2 rounded-full', DOT_CLASS[tone])} />
      <Text variant="label" numberOfLines={1}>
        {label}
      </Text>
    </Material>
  );
}

export { StatusPill };
export type { StatusPillProps, StatusTone };
