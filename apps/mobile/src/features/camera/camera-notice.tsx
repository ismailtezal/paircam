import type { DisconnectReason, EngineEnvironment, EngineState } from 'camera-engine';
import * as React from 'react';
import { useTranslation } from 'react-i18next';
import Animated, { FadeInUp, FadeOutUp } from 'react-native-reanimated';

import { Notice, type NoticeProps } from '@/components/ui/notice';
import { engine, type Disconnect } from '@/engine/store';
import { motion } from '@/theme';

import { openAppSettings } from './permission-blocked';

/** How long a notice about a session that has ended stays up. */
const ENDED_NOTICE_VISIBLE_MS = 6000;

/** Endings the user did not choose and should be told about. */
const REPORTED_ENDINGS: ReadonlySet<DisconnectReason> = new Set([
  'lost',
  'hostEnded',
  'pairingTimedOut',
]);

type CameraNoticeProps = {
  state: EngineState;
  environment: EngineEnvironment;
  disconnect: Disconnect | null;
};

/**
 * The one thing, if any, that is stopping the camera from doing its job, most serious first:
 *
 * 1. Local Network access is off: the computer cannot find the phone at all.
 * 2. No Wi-Fi, while waiting for a computer.
 * 3. Something else has taken the camera.
 * 4. The last session ended without the user asking. This one clears itself, because the app
 *    has already gone back to waiting and there is nothing to do.
 */
export function CameraNotice({ state, environment, disconnect }: CameraNoticeProps) {
  const { t } = useTranslation();

  React.useEffect(() => {
    if (!disconnect) {
      return;
    }
    if (!REPORTED_ENDINGS.has(disconnect.reason)) {
      engine.acknowledgeDisconnect();
      return;
    }
    const timer = setTimeout(engine.acknowledgeDisconnect, ENDED_NOTICE_VISIBLE_MS);
    return () => clearTimeout(timer);
  }, [disconnect]);

  const waiting = state.status === 'idle' || state.status === 'advertising';
  let notice: (NoticeProps & { key: string }) | null = null;

  if (environment.localNetwork === 'denied') {
    notice = {
      key: 'local-network',
      tone: 'warning',
      icon: 'network.slash',
      title: t('permission.localNetworkDenied.title'),
      body: t('permission.localNetworkDenied.body'),
      action: { label: t('common.openSettings'), onPress: openAppSettings },
    };
  } else if (waiting && !environment.wifi) {
    notice = {
      key: 'wifi',
      tone: 'warning',
      icon: 'wifi.slash',
      title: t('notice.noWifi.title'),
      body: t('notice.noWifi.body'),
    };
  } else if (environment.cameraInterrupted) {
    notice = {
      key: 'interrupted',
      icon: 'pause.circle',
      title: t('notice.cameraInterrupted.title'),
      body: t('notice.cameraInterrupted.body'),
    };
  } else if (disconnect && waiting && REPORTED_ENDINGS.has(disconnect.reason)) {
    const host = disconnect.hostName;
    if (disconnect.reason === 'lost') {
      notice = {
        key: `ended-${disconnect.id}`,
        tone: 'danger',
        icon: 'wifi.exclamationmark',
        title: t('notice.connectionLost.title'),
        body: t('notice.connectionLost.body', { host }),
      };
    } else if (disconnect.reason === 'pairingTimedOut') {
      notice = {
        key: `ended-${disconnect.id}`,
        icon: 'clock',
        title: t('notice.pairingTimedOut.title'),
        body: t('notice.pairingTimedOut.body', { host }),
      };
    } else {
      notice = {
        key: `ended-${disconnect.id}`,
        icon: 'xmark.circle',
        title: t('notice.hostEnded.title'),
        body: t('notice.hostEnded.body', { host }),
      };
    }
  }

  if (!notice) {
    return null;
  }
  const { key, ...props } = notice;
  // The notice drops in from under the status bar and leaves the way it came.
  return (
    <Animated.View
      key={key}
      entering={FadeInUp.duration(motion.duration.base).easing(motion.easing.out)}
      exiting={FadeOutUp.duration(motion.duration.fast).easing(motion.easing.out)}
    >
      <Notice {...props} />
    </Animated.View>
  );
}
