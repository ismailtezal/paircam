import * as React from 'react';
import { Switch as NativeSwitch } from 'react-native';

import { haptics } from '@/lib/haptics';
import { useTheme } from '@/theme';

type SwitchProps = {
  value: boolean;
  onValueChange: (value: boolean) => void;
  /** Required: the switch alone does not say what it turns on. */
  accessibilityLabel: string;
  disabled?: boolean;
};

/**
 * An on/off control. It is the system's own switch, so it moves, sounds and reads to VoiceOver
 * exactly as iOS users expect; only the "on" track takes the accent colour.
 */
function Switch({ value, onValueChange, accessibilityLabel, disabled }: SwitchProps) {
  const { colors } = useTheme();
  return (
    <NativeSwitch
      value={value}
      onValueChange={(next) => {
        haptics.selection();
        onValueChange(next);
      }}
      trackColor={{ true: colors.accent }}
      accessibilityLabel={accessibilityLabel}
      disabled={disabled}
    />
  );
}

export { Switch };
export type { SwitchProps };
