import type { CameraPermission } from 'camera-engine';
import * as React from 'react';
import { useTranslation } from 'react-i18next';
import { Linking, StyleSheet, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { Notice } from '@/components/ui/notice';
import { SHEET_MAX_WIDTH } from '@/components/ui/sheet';
import { tokens } from '@/theme';

type PermissionBlockedProps = {
  cameraPermission: Extract<CameraPermission, 'denied' | 'restricted'>;
};

/** Opens PairCam's page in the Settings app, where the permission switches live. */
export function openAppSettings() {
  Linking.openSettings().catch(() => {
    // Settings could not be opened; the notice already tells the user where to go.
  });
}

/**
 * The camera is off limits. There is no preview to show, so the explanation takes its place:
 * what is wrong, why it matters, and the one button that fixes it. A restricted camera (Screen
 * Time, device management) cannot be fixed from the app's settings page, so it gets no button.
 */
export function PermissionBlocked({ cameraPermission }: PermissionBlockedProps) {
  const { t } = useTranslation();
  const insets = useSafeAreaInsets();
  const denied = cameraPermission === 'denied';

  return (
    <View
      style={[
        StyleSheet.absoluteFill,
        styles.backdrop,
        {
          paddingTop: insets.top + tokens.space[4],
          paddingBottom: insets.bottom + tokens.space[4],
          paddingLeft: insets.left + tokens.space[4],
          paddingRight: insets.right + tokens.space[4],
        },
      ]}
    >
      <View style={styles.card}>
        <Notice
          tone="warning"
          icon="video.slash"
          title={t(denied ? 'permission.cameraDenied.title' : 'permission.cameraRestricted.title')}
          body={t(denied ? 'permission.cameraDenied.body' : 'permission.cameraRestricted.body')}
          action={
            denied ? { label: t('common.openSettings'), onPress: openAppSettings } : undefined
          }
        />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  backdrop: {
    justifyContent: 'center',
    alignItems: 'center',
  },
  card: {
    width: '100%',
    maxWidth: SHEET_MAX_WIDTH,
  },
});
