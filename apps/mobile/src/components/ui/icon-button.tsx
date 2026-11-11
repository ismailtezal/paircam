import * as React from 'react';
import { View } from 'react-native';

import { cn } from '@/lib/utils';

import { Icon, type IconName } from './icon';
import { Material } from './material';
import { PressableScale, type PressableScaleProps } from './pressable-scale';

type IconButtonProps = Omit<PressableScaleProps, 'children'> & {
  icon: IconName;
  /** Required: an icon alone says nothing to VoiceOver. */
  accessibilityLabel: string;
  /** `material` floats over the camera preview; `plain` sits on a solid surface. */
  variant?: 'material' | 'plain';
};

/** A round, 44 pt icon control. */
function IconButton({ icon, variant = 'material', ...props }: IconButtonProps) {
  return (
    <PressableScale accessibilityRole="button" {...props}>
      {(pressed) =>
        variant === 'material' ? (
          <Material className="size-touch-target items-center justify-center rounded-full">
            {pressed ? <View className="absolute inset-0 bg-surface-active" /> : null}
            <Icon name={icon} />
          </Material>
        ) : (
          <View
            className={cn(
              'size-touch-target items-center justify-center rounded-full',
              pressed && 'bg-surface-active',
            )}
          >
            <Icon name={icon} />
          </View>
        )
      }
    </PressableScale>
  );
}

export { IconButton };
export type { IconButtonProps };
