import type { EngineState } from 'camera-engine';
import * as React from 'react';
import { useTranslation } from 'react-i18next';
import { View } from 'react-native';
import Animated, { FadeIn } from 'react-native-reanimated';

import { Button } from '@/components/ui/button';
import { Material } from '@/components/ui/material';
import { Text } from '@/components/ui/text';
import { engine } from '@/engine/store';
import { motion } from '@/theme';

import { StreamStats } from './stream-stats';

type SessionPanelProps = {
  state: EngineState;
  deviceName: string;
  onDim: () => void;
};

/**
 * The panel at the bottom of the camera screen. It says what is happening in a sentence and
 * offers the one thing the user can do about it:
 *
 * - waiting: how to connect (nothing to press; the computer does the connecting);
 * - connected: disconnect;
 * - streaming: the live numbers, dim the screen, and Stop as the single primary action.
 *
 * When the state changes, the new content fades in quickly. A change of state is occasional and
 * comes from the computer, so a short fade is enough to keep it from snapping.
 */
export function SessionPanel({ state, deviceName, onDim }: SessionPanelProps) {
  const { t } = useTranslation();
  // Waiting and pairing show the same content; only a real change of content should fade.
  const content =
    state.status === 'streaming' || state.status === 'connected' ? state.status : 'ready';

  return (
    <Material className="p-5">
      <Animated.View key={content} entering={FadeIn.duration(motion.duration.base)}>
        {state.status === 'streaming' ? (
          <View className="gap-5">
            <Text variant="headline" numberOfLines={1}>
              {t('streaming.title', { host: state.hostName })}
            </Text>
            <StreamStats stream={state} />
            <View className="flex-row gap-3">
              <View className="flex-1">
                <Button
                  variant="secondary"
                  label={t('streaming.dim')}
                  haptic="light"
                  onPress={onDim}
                />
              </View>
              <View className="flex-1">
                <Button
                  variant="danger"
                  label={t('streaming.stop')}
                  haptic="medium"
                  onPress={engine.disconnect}
                />
              </View>
            </View>
          </View>
        ) : state.status === 'connected' ? (
          <View className="gap-5">
            <View className="gap-1">
              <Text variant="headline" numberOfLines={1}>
                {t('connected.title', { host: state.hostName })}
              </Text>
              <Text tone="muted">{t('connected.hint')}</Text>
            </View>
            <Button
              variant="secondary"
              label={t('connected.disconnect')}
              haptic="light"
              onPress={engine.disconnect}
            />
          </View>
        ) : (
          <View className="gap-1">
            <Text variant="headline" numberOfLines={1}>
              {deviceName}
            </Text>
            <Text tone="muted">{t('ready.hint')}</Text>
          </View>
        )}
      </Animated.View>
    </Material>
  );
}
