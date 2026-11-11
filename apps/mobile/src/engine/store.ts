import {
  CameraEngine,
  type DisconnectEvent,
  type EngineEnvironment,
  type EngineState,
  type GestureEvent,
  type StreamStats,
} from 'camera-engine';
import { useSyncExternalStore } from 'react';

// The interface's view of the native engine: one snapshot, kept current by the engine's events
// and read with `useEngine`. The engine itself lives in Swift and keeps running whether or not
// any component is mounted.

export type Disconnect = DisconnectEvent & {
  /** Distinguishes two identical events in a row. */
  id: number;
};

export type Gesture = GestureEvent & {
  /** Distinguishes two identical gestures in a row. */
  id: number;
};

export type EngineSnapshot = {
  state: EngineState;
  environment: EngineEnvironment;
  /** The latest stats while streaming, otherwise `null`. */
  stats: StreamStats | null;
  /** How the last session ended, until it is acknowledged or a new session begins. */
  disconnect: Disconnect | null;
  /** The last hand gesture the camera acted on in this session, or `null`. */
  gesture: Gesture | null;
};

let snapshot: EngineSnapshot = {
  state: CameraEngine.getState(),
  environment: CameraEngine.getEnvironment(),
  stats: null,
  disconnect: null,
  gesture: null,
};

const listeners = new Set<() => void>();
let subscribed = false;
let disconnectCount = 0;
let gestureCount = 0;

function update(change: Partial<EngineSnapshot>) {
  snapshot = { ...snapshot, ...change };
  listeners.forEach((listener) => listener());
}

function subscribeToEngine() {
  if (subscribed) {
    return;
  }
  subscribed = true;

  CameraEngine.addListener('onStateChange', (state) => {
    const active = state.status !== 'idle' && state.status !== 'advertising';
    update({
      state,
      stats: state.status === 'streaming' ? snapshot.stats : null,
      // A new session supersedes the story of how the previous one ended.
      disconnect: active ? null : snapshot.disconnect,
      gesture: active ? snapshot.gesture : null,
    });
  });
  CameraEngine.addListener('onEnvironmentChange', (environment) => {
    update({ environment });
  });
  CameraEngine.addListener('onStats', (stats) => {
    update({ stats });
  });
  CameraEngine.addListener('onDisconnect', (event) => {
    disconnectCount += 1;
    update({ disconnect: { ...event, id: disconnectCount } });
  });
  CameraEngine.addListener('onGesture', (event) => {
    gestureCount += 1;
    update({ gesture: { ...event, id: gestureCount } });
  });

  // Events that fired before the listeners were attached are covered by reading once more.
  update({
    state: CameraEngine.getState(),
    environment: CameraEngine.getEnvironment(),
  });
}

function subscribe(listener: () => void) {
  subscribeToEngine();
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Reads part of the engine snapshot and re-renders when that part changes. Select the smallest
 * piece a component needs: stats change every second, state rarely.
 */
export function useEngine<T>(select: (snapshot: EngineSnapshot) => T): T {
  return useSyncExternalStore(subscribe, () => select(snapshot));
}

const ignoreRejection = () => {
  // Engine commands only reject if the native module is gone, which cannot happen at runtime.
};

/** Commands for the engine. They return immediately; results arrive as snapshot changes. */
export const engine = {
  /** Starts the camera and advertising. The first call makes iOS ask for Local Network access. */
  start() {
    CameraEngine.start().catch(ignoreRejection);
  },
  requestCameraPermission() {
    return CameraEngine.requestCameraPermission();
  },
  respondToPairing(accept: boolean) {
    CameraEngine.respondToPairing(accept).catch(ignoreRejection);
  },
  disconnect() {
    CameraEngine.disconnect().catch(ignoreRejection);
  },
  pairedHosts() {
    return CameraEngine.getPairedHosts();
  },
  forgetHost(id: string) {
    return CameraEngine.forgetHost(id);
  },
  /** The user's choice in Settings; applied when a session starts, and to one already running. */
  setFaceTracking(enabled: boolean) {
    CameraEngine.setFaceTracking(enabled).catch(ignoreRejection);
  },
  setGestures(enabled: boolean) {
    CameraEngine.setGestures(enabled).catch(ignoreRejection);
  },
  /** Clears the record of how the last session ended, once the user has seen it. */
  acknowledgeDisconnect() {
    if (snapshot.disconnect) {
      update({ disconnect: null });
    }
  },
};
