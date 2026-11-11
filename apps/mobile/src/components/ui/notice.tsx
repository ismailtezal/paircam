import * as React from 'react';
import { View } from 'react-native';

import { Button } from './button';
import { Icon, type IconName } from './icon';
import { Material } from './material';
import { Text } from './text';

type NoticeTone = 'info' | 'warning' | 'danger';

const ICON_COLOR = {
  info: 'textMuted',
  warning: 'warning',
  danger: 'danger',
} as const;

type NoticeProps = {
  tone?: NoticeTone;
  icon: IconName;
  title: string;
  body: string;
  /** One way out of the problem, if the user can do something about it. */
  action?: {
    label: string;
    onPress: () => void;
  };
};

/**
 * Says what is wrong, why, and what to do about it, in a card that floats over the preview.
 * Used for the states where the camera cannot do its job: no Wi-Fi, a lost connection, a
 * permission that is off.
 */
function Notice({ tone = 'info', icon, title, body, action }: NoticeProps) {
  return (
    <Material
      className="gap-3 p-4"
      accessible={action === undefined}
      accessibilityRole="alert"
      accessibilityLabel={action === undefined ? `${title}. ${body}` : undefined}
    >
      <View className="flex-row gap-3">
        <View className="pt-0.5">
          <Icon name={icon} color={ICON_COLOR[tone]} />
        </View>
        <View className="flex-1 gap-0.5">
          <Text variant="headline">{title}</Text>
          <Text tone="muted">{body}</Text>
        </View>
      </View>
      {action ? (
        <Button variant="secondary" size="md" label={action.label} onPress={action.onPress} />
      ) : null}
    </Material>
  );
}

export { Notice };
export type { NoticeProps, NoticeTone };
