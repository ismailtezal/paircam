import type { CameraPermission } from 'camera-engine';
import * as React from 'react';
import { useTranslation } from 'react-i18next';
import { StyleSheet, View } from 'react-native';
import Animated, { FadeOut } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { Button } from '@/components/ui/button';
import { Icon, type IconName } from '@/components/ui/icon';
import { Material } from '@/components/ui/material';
import { SHEET_MAX_WIDTH } from '@/components/ui/sheet';
import { Text } from '@/components/ui/text';
import { engine } from '@/engine/store';
import { cn } from '@/lib/utils';
import { motion, tokens } from '@/theme';

const TOTAL_STEPS = 2;

type SetupOverlayProps = {
  cameraPermission: CameraPermission;
  /** The user has been through both steps. The caller starts the engine, which makes iOS ask
   * for Local Network access. */
  onComplete: () => void;
};

/**
 * First run. Two permissions, each explained in a sentence before the system asks for it, one
 * step at a time with a single button.
 *
 * The camera comes first so that the moment it is allowed, the live preview appears behind the
 * card: the user sees what the app is for before the second question.
 */
export function SetupOverlay({ cameraPermission, onComplete }: SetupOverlayProps) {
  const { t } = useTranslation();
  const insets = useSafeAreaInsets();
  const [requesting, setRequesting] = React.useState(false);

  const cameraAllowed = cameraPermission === 'granted';
  const step = cameraAllowed ? 2 : 1;

  const allowCamera = async () => {
    setRequesting(true);
    try {
      await engine.requestCameraPermission();
    } finally {
      setRequesting(false);
    }
  };

  return (
    <Animated.View style={StyleSheet.absoluteFill} exiting={FadeOut.duration(motion.duration.base)}>
      <View
        className={cn('flex-1 justify-end', cameraAllowed ? 'bg-overlay' : 'bg-transparent')}
        style={{
          paddingTop: insets.top + tokens.space[4],
          paddingBottom: Math.max(insets.bottom, tokens.space[3]),
          paddingLeft: Math.max(insets.left, tokens.space[3]),
          paddingRight: Math.max(insets.right, tokens.space[3]),
        }}
      >
        <Material className="w-full gap-6 self-center p-6" style={styles.card}>
          <View className="gap-2">
            <Text variant="label" tone="muted">
              {t('setup.step', { current: step, total: TOTAL_STEPS })}
            </Text>
            <Text variant="display" accessibilityRole="header">
              {t('setup.title')}
            </Text>
            <Text tone="muted">{t('setup.subtitle')}</Text>
          </View>

          <View className="gap-4">
            <PermissionRow
              icon="camera"
              title={t('setup.camera.title')}
              body={t('setup.camera.body')}
              allowed={cameraAllowed}
              allowedLabel={t('setup.allowed')}
              dimmed={false}
            />
            <PermissionRow
              icon="network"
              title={t('setup.localNetwork.title')}
              body={t('setup.localNetwork.body')}
              allowed={false}
              allowedLabel={t('setup.allowed')}
              dimmed={!cameraAllowed}
            />
          </View>

          {cameraAllowed ? (
            <Button label={t('setup.localNetwork.action')} haptic="light" onPress={onComplete} />
          ) : (
            <Button
              label={t('setup.camera.action')}
              haptic="light"
              loading={requesting}
              onPress={allowCamera}
            />
          )}
        </Material>
      </View>
    </Animated.View>
  );
}

type PermissionRowProps = {
  icon: IconName;
  title: string;
  body: string;
  allowed: boolean;
  allowedLabel: string;
  /** Not this step's turn yet. */
  dimmed: boolean;
};

function PermissionRow({ icon, title, body, allowed, allowedLabel, dimmed }: PermissionRowProps) {
  return (
    <View
      className={cn('flex-row gap-3', dimmed && 'opacity-50')}
      accessible
      accessibilityLabel={allowed ? `${title}, ${allowedLabel}` : `${title}. ${body}`}
    >
      <View className="size-touch-target items-center justify-center rounded-lg border-hairline border-border bg-surface">
        <Icon name={icon} />
      </View>
      <View className="flex-1 justify-center gap-0.5">
        <Text variant="headline">{title}</Text>
        <Text tone="muted">{body}</Text>
      </View>
      {allowed ? (
        <View className="justify-center">
          <Icon name="checkmark.circle.fill" color="live" />
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  card: {
    maxWidth: SHEET_MAX_WIDTH,
  },
});
