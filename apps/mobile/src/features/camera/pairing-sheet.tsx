import * as React from 'react';
import { useTranslation } from 'react-i18next';
import { View } from 'react-native';

import { Button } from '@/components/ui/button';
import { Sheet } from '@/components/ui/sheet';
import { Text } from '@/components/ui/text';
import { engine } from '@/engine/store';

/** The code is already the largest text in the app; let Dynamic Type grow it only a little. */
const CODE_MAX_FONT_SCALE = 1.2;

type PairingSheetProps = {
  hostName: string;
  /** Six digits. */
  code: string;
};

/**
 * A computer is asking to pair. The user compares the code with the one on the computer, which
 * is what stops another device on the network from pairing in its place, and then decides.
 *
 * The code is the content: large, mono, split in two groups of three so it can be read across a
 * desk. Allow is the single primary action; Decline is beside it and quieter.
 */
export function PairingSheet({ hostName, code }: PairingSheetProps) {
  const { t } = useTranslation();
  const groups = [code.slice(0, 3), code.slice(3)];

  return (
    <Sheet accessibilityLabel={t('pairing.title', { host: hostName })}>
      <View className="gap-6">
        <View className="gap-2">
          <Text variant="display" accessibilityRole="header">
            {t('pairing.title', { host: hostName })}
          </Text>
          <Text tone="muted">{t('pairing.body')}</Text>
        </View>

        <View
          className="flex-row justify-center gap-4 rounded-lg border-hairline border-border bg-surface py-5"
          accessible
          accessibilityLabel={t('pairing.codeLabel', { code: code.split('').join(' ') })}
        >
          {groups.map((group, index) => (
            <Text key={index} variant="code" maxFontSizeMultiplier={CODE_MAX_FONT_SCALE}>
              {group}
            </Text>
          ))}
        </View>

        <View className="flex-row gap-3">
          <View className="flex-1">
            <Button
              variant="secondary"
              label={t('pairing.decline')}
              haptic="light"
              onPress={() => engine.respondToPairing(false)}
            />
          </View>
          <View className="flex-1">
            <Button
              label={t('pairing.allow')}
              haptic="success"
              onPress={() => engine.respondToPairing(true)}
            />
          </View>
        </View>
      </View>
    </Sheet>
  );
}
