import { requireNativeView } from 'expo';
import type { ComponentType } from 'react';

import type { CameraPreviewProps } from './CameraEngine.types';

const NativeCameraPreview: ComponentType<CameraPreviewProps> = requireNativeView('CameraEngine');

/**
 * The live camera picture, drawn natively from the engine's capture session. React Native only
 * lays it out; no frame crosses into JavaScript.
 */
export function CameraPreview({ active = true, style }: CameraPreviewProps) {
  return <NativeCameraPreview active={active} style={style} />;
}
