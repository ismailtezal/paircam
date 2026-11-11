import { NativeModule, requireNativeModule } from 'expo';

import type {
  CameraEngineEvents,
  CameraPermission,
  EngineEnvironment,
  EngineState,
  PairedHost,
} from './CameraEngine.types';

declare class CameraEngineNativeModule extends NativeModule<CameraEngineEvents> {
  /** The current state, without waiting for an event. */
  getState(): EngineState;
  getEnvironment(): EngineEnvironment;

  /**
   * Starts the camera and advertises the phone on the local network. The first call makes iOS ask
   * for Local Network access. Calling it again is harmless.
   */
  start(): Promise<void>;
  stop(): Promise<void>;

  /** The user's answer to a pairing request, after comparing the code with the computer. */
  respondToPairing(accept: boolean): Promise<void>;

  /** Ends the current session and goes back to waiting for a computer. */
  disconnect(): Promise<void>;

  /**
   * The user's choice of face tracking in Settings. Applied when a session starts, and at once to
   * a session that is already running.
   */
  setFaceTracking(enabled: boolean): Promise<void>;
  /** The user's choice of hand gestures in Settings, applied like `setFaceTracking`. */
  setGestures(enabled: boolean): Promise<void>;

  getPairedHosts(): Promise<PairedHost[]>;
  forgetHost(id: string): Promise<void>;

  /** Shows the system camera prompt if the user has not answered it yet. */
  requestCameraPermission(): Promise<CameraPermission>;
}

export default requireNativeModule<CameraEngineNativeModule>('CameraEngine');
