import * as React from 'react';
import { Pressable, StyleSheet, View } from 'react-native';

import { cn } from '@/lib/utils';

import { Icon } from './icon';
import { Text } from './text';

type ListSectionProps = {
  title?: string;
  /** A sentence under the group that explains what its rows do. */
  footer?: string;
  children: React.ReactNode;
};

/** A titled group of rows on a solid surface, separated by hairlines. */
function ListSection({ title, footer, children }: ListSectionProps) {
  const rows = React.Children.toArray(children);
  return (
    <View className="gap-2">
      {title ? (
        <Text variant="label" tone="muted" className="px-4" accessibilityRole="header">
          {title}
        </Text>
      ) : null}
      <View
        className="overflow-hidden rounded-lg border-hairline border-border bg-surface"
        style={styles.continuous}
      >
        {rows.map((row, index) => (
          <React.Fragment key={index}>
            {index > 0 ? <View className="ml-4 border-b-hairline border-border" /> : null}
            {row}
          </React.Fragment>
        ))}
      </View>
      {footer ? (
        <Text variant="caption" tone="subtle" className="px-4">
          {footer}
        </Text>
      ) : null}
    </View>
  );
}

type ListRowProps = {
  title: string;
  subtitle?: string;
  /** The current value, shown at the trailing edge. */
  value?: string;
  /** Sets the value in mono, for versions and other machine text. */
  monoValue?: boolean;
  /** Marks the chosen option in a list of choices. */
  selected?: boolean;
  /** Shows a chevron: the row opens another screen. */
  navigates?: boolean;
  /** A control at the trailing edge, such as a button. */
  trailing?: React.ReactNode;
  onPress?: () => void;
  accessibilityLabel?: string;
};

/**
 * One row. With `onPress` the whole row is the target and darkens the instant it is touched;
 * rows are used too often for anything slower than that.
 */
function ListRow({
  title,
  subtitle,
  value,
  monoValue = false,
  selected,
  navigates = false,
  trailing,
  onPress,
  accessibilityLabel,
}: ListRowProps) {
  const content = (pressed: boolean) => (
    <View
      className={cn(
        'min-h-touch-target flex-row items-center gap-3 px-4 py-3',
        pressed && 'bg-surface-active',
      )}
    >
      <View className="flex-1 gap-0.5">
        <Text numberOfLines={1}>{title}</Text>
        {subtitle ? (
          <Text variant="caption" tone="muted" numberOfLines={1}>
            {subtitle}
          </Text>
        ) : null}
      </View>
      {value ? (
        <Text variant={monoValue ? 'mono' : 'body'} tone="muted" numberOfLines={1}>
          {value}
        </Text>
      ) : null}
      {trailing}
      {selected ? <Icon name="checkmark" size="sm" color="accent" weight="semibold" /> : null}
      {navigates ? (
        <Icon name="chevron.right" size="sm" color="textSubtle" weight="semibold" />
      ) : null}
    </View>
  );

  if (!onPress) {
    return content(false);
  }
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel ?? (value ? `${title}, ${value}` : title)}
      accessibilityState={selected === undefined ? undefined : { selected }}
    >
      {({ pressed }) => content(pressed)}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  continuous: {
    borderCurve: 'continuous',
  },
});

export { ListRow, ListSection };
export type { ListRowProps, ListSectionProps };
