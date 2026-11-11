// The camera engine: a native Swift module that owns the camera, the hardware encoder and the
// connection to the desktop app. This is its whole JavaScript surface, and the seam where an
// engine for another platform would plug in.

export { default as CameraEngine } from './src/CameraEngineModule';
export { CameraPreview } from './src/CameraPreview';
export type * from './src/CameraEngine.types';
