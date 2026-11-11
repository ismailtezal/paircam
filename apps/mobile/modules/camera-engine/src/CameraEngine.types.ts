import type { StyleProp, ViewStyle } from 'react-native';

export type VideoCodec = 'hevc' | 'h264';

/** What the engine is doing. Mirrors `EngineState` in `ios/Engine/EngineState.swift`. */
export type EngineState =
  /** The service is stopped. */
  | { status: 'idle' }
  /** Waiting for a computer to connect. */
  | { status: 'advertising' }
  /** A computer wants to pair. The user compares `code` with the computer and decides. */
  | { status: 'pairing'; hostName: string; code: string }
  /** A paired computer is connected but not streaming. */
  | { status: 'connected'; hostName: string }
  | {
      status: 'streaming';
      hostName: string;
      /** Encoded picture size, after rotation. */
      width: number;
      height: number;
      fps: number;
      codec: VideoCodec;
    };

export type EngineStatus = EngineState['status'];

export type ThermalState = 'nominal' | 'fair' | 'serious' | 'critical' | 'unknown';

/** Sent about once a second while streaming. */
export type StreamStats = {
  /** Frames per second actually sent. */
  fps: number;
  /** Bits per second actually produced by the encoder. */
  bitrate: number;
  /** Frames dropped since the stream started because the connection fell behind. */
  droppedFrames: number;
  thermal: ThermalState;
  /** Battery level from 0 to 1. */
  battery: number;
  charging: boolean;
};

export type CameraPermission = 'undetermined' | 'granted' | 'denied' | 'restricted';

/**
 * iOS has no API to ask about Local Network access. It stays `unknown` until the service has been
 * started and the system has either advertised the camera (`granted`) or refused (`denied`).
 */
export type LocalNetworkPermission = 'unknown' | 'granted' | 'denied';

/** Things around the engine that the interface explains to the user. */
export type EngineEnvironment = {
  cameraPermission: CameraPermission;
  localNetwork: LocalNetworkPermission;
  /** The phone is on Wi-Fi, so a computer on the same network can reach it. */
  wifi: boolean;
  /** Another app or the system has taken the camera away for the moment. */
  cameraInterrupted: boolean;
  deviceName: string;
  /** The TCP port the service listens on, or 0 while it is stopped. */
  port: number;
};

export type PairedHost = {
  /** The computer's device id, 32 hex characters. */
  id: string;
  name: string;
  /** When the computer was paired, in milliseconds since the Unix epoch. */
  pairedAt?: number;
};

export type DisconnectReason =
  /** The user pressed Stop on the phone. */
  | 'stopped'
  /** The user declined a pairing request. */
  | 'declined'
  /** A pairing request was left unanswered. */
  | 'pairingTimedOut'
  /** The app went to the background, where iOS stops the camera. */
  | 'backgrounded'
  /** The connected computer was removed from the paired list. */
  | 'forgotten'
  /** The computer ended the session. */
  | 'hostEnded'
  /** The connection dropped without a goodbye. */
  | 'lost';

export type DisconnectEvent = {
  reason: DisconnectReason;
  hostName: string;
  during: 'pairing' | 'connected' | 'streaming';
};

export type CameraEngineEvents = {
  onStateChange: (state: EngineState) => void;
  onEnvironmentChange: (environment: EngineEnvironment) => void;
  onStats: (stats: StreamStats) => void;
  onDisconnect: (event: DisconnectEvent) => void;
};

export type CameraPreviewProps = {
  /** Set to `false` to stop drawing while the screen is dimmed. Streaming is not affected. */
  active?: boolean;
  style?: StyleProp<ViewStyle>;
};
