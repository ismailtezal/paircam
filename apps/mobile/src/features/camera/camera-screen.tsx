import { CameraPreview, type EngineState } from 'camera-engine';
import { activateKeepAwakeAsync, deactivateKeepAwake } from 'expo-keep-awake';
import { useRouter } from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import * as React from 'react';
import { useTranslation } from 'react-i18next';
import { StyleSheet, useWindowDimensions, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { IconButton } from '@/components/ui/icon-button';
import { SHEET_MAX_WIDTH } from '@/components/ui/sheet';
import { StatusPill, type StatusTone } from '@/components/ui/status-pill';
import { engine, useEngine } from '@/engine/store';
import { haptics } from '@/lib/haptics';
import { readSetting, writeSetting } from '@/lib/storage';
import { tokens } from '@/theme';

import { CameraNotice } from './camera-notice';
import { DimOverlay } from './dim-overlay';
import { PairingSheet } from './pairing-sheet';
import { PermissionBlocked } from './permission-blocked';
import { SessionPanel } from './session-panel';
import { SetupOverlay } from './setup-overlay';

const KEEP_AWAKE_TAG = 'paircam-session';

// Behind the preview it is always black, in both themes, like a viewfinder.
const BACKDROP = tokens.color.dark.background;

function statusTone(state: EngineState, wifi: boolean): StatusTone {
  switch (state.status) {
    case 'streaming':
      return 'live';
    case 'connected':
      return 'active';
    case 'pairing':
      return 'idle';
    case 'idle':
    case 'advertising':
      return wifi ? 'idle' : 'warning';
  }
}

/**
 * The app's one real screen. The live preview fills it edge to edge, and everything else floats
 * over the picture in translucent controls: where things stand (top left), settings (top right),
 * anything that is wrong (under those), and what is happening with the one thing to do about it
 * (bottom).
 *
 * The preview is never unmounted between states, so pairing, connecting and streaming never make
 * the picture blink.
 */
export function CameraScreen() {
  const { t } = useTranslation();
  const router = useRouter();
  const insets = useSafeAreaInsets();
  const { width, height } = useWindowDimensions();

  const state = useEngine((snapshot) => snapshot.state);
  const environment = useEngine((snapshot) => snapshot.environment);
  const disconnect = useEngine((snapshot) => snapshot.disconnect);

  const [setupComplete, setSetupComplete] = React.useState(
    () => readSetting('onboardingComplete') === 'true',
  );
  const [dimRequested, setDimRequested] = React.useState(false);

  const cameraPermission = environment.cameraPermission;
  const cameraAllowed = cameraPermission === 'granted';
  const streaming = state.status === 'streaming';
  const sessionActive = state.status !== 'idle' && state.status !== 'advertising';
  const ready = setupComplete && cameraAllowed;

  // Dimming belongs to one stream: when the stream ends, the next one starts with the screen on.
  const [wasStreaming, setWasStreaming] = React.useState(streaming);
  if (wasStreaming !== streaming) {
    setWasStreaming(streaming);
    if (!streaming) {
      setDimRequested(false);
    }
  }
  const dimmed = dimRequested && streaming;

  // Once set up, the engine runs for as long as the app is open. It pauses itself in the
  // background and resumes in the foreground.
  React.useEffect(() => {
    if (ready) {
      engine.start();
    }
  }, [ready]);

  // The phone must not lock while a computer is using it: iOS would stop the camera.
  React.useEffect(() => {
    if (!sessionActive) {
      return;
    }
    activateKeepAwakeAsync(KEEP_AWAKE_TAG).catch(() => {
      // Without keep-awake the phone follows the user's auto-lock setting.
    });
    return () => {
      deactivateKeepAwake(KEEP_AWAKE_TAG).catch(() => {});
    };
  }, [sessionActive]);

  // A pairing request needs attention even if the phone is on a stand across the desk.
  const pairing = state.status === 'pairing';
  React.useEffect(() => {
    if (pairing) {
      haptics.warning();
    }
  }, [pairing]);

  const lostConnection = disconnect?.reason === 'lost' ? disconnect.id : null;
  React.useEffect(() => {
    if (lostConnection !== null) {
      haptics.error();
    }
  }, [lostConnection]);

  const completeSetup = () => {
    writeSetting('onboardingComplete', 'true');
    setSetupComplete(true);
  };

  const statusLabel =
    state.status === 'streaming'
      ? t('streaming.status')
      : state.status === 'connected'
        ? t('connected.status')
        : state.status === 'idle'
          ? t('ready.starting')
          : t('ready.status');

  const landscape = width > height;
  const gutter = tokens.space[4];

  return (
    <View style={styles.screen}>
      <StatusBar style="light" hidden={dimmed} animated />
      <CameraPreview style={StyleSheet.absoluteFill} active={!dimmed} />

      {ready ? (
        <View
          style={[
            StyleSheet.absoluteFill,
            {
              paddingTop: insets.top + tokens.space[2],
              paddingBottom: Math.max(insets.bottom, gutter),
              paddingLeft: Math.max(insets.left, gutter),
              paddingRight: Math.max(insets.right, gutter),
            },
          ]}
          pointerEvents="box-none"
        >
          <View className="flex-row items-start justify-between gap-3" pointerEvents="box-none">
            <StatusPill
              tone={statusTone(state, environment.wifi)}
              label={statusLabel}
              className="shrink"
            />
            <IconButton
              icon="gearshape"
              accessibilityLabel={t('settings.title')}
              onPress={() => router.push('/settings')}
            />
          </View>

          <View style={styles.column} className="mt-3 w-full" pointerEvents="box-none">
            <CameraNotice state={state} environment={environment} disconnect={disconnect} />
          </View>

          <View className="flex-1" pointerEvents="none" />

          <View
            style={styles.column}
            className={landscape ? 'w-full self-start' : 'w-full self-center'}
          >
            <SessionPanel
              state={state}
              deviceName={environment.deviceName}
              onDim={() => setDimRequested(true)}
            />
          </View>
        </View>
      ) : null}

      {cameraPermission === 'denied' || cameraPermission === 'restricted' ? (
        <PermissionBlocked cameraPermission={cameraPermission} />
      ) : !ready ? (
        <SetupOverlay cameraPermission={cameraPermission} onComplete={completeSetup} />
      ) : null}

      {state.status === 'pairing' ? (
        <PairingSheet hostName={state.hostName} code={state.code} />
      ) : null}

      {dimmed ? <DimOverlay onWake={() => setDimRequested(false)} /> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: BACKDROP,
  },
  column: {
    maxWidth: SHEET_MAX_WIDTH,
  },
});
