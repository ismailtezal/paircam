// Lets this file import other TypeScript files (the design tokens) when Expo evaluates it.
import 'tsx/cjs';

import type { ConfigContext, ExpoConfig } from 'expo/config';

import { tokens } from './src/theme/tokens';

// The bundle identifier is a placeholder until the owner confirms it. Changing it later means a
// new app record in App Store Connect, so settle it before the first TestFlight build.
const BUNDLE_IDENTIFIER = 'app.paircam.ios';

// The EAS project this app builds under. `eas init` prints the id; paste it here, because EAS
// cannot write into a TypeScript config by itself.
const EAS_PROJECT_ID: string | undefined = undefined;

// The Bonjour service the camera advertises. See "Discovery" in docs/PROTOCOL.md.
const BONJOUR_SERVICE = '_paircam._tcp';

export default ({ config }: ConfigContext): ExpoConfig => ({
  ...config,
  name: 'PairCam',
  slug: 'paircam',
  scheme: 'paircam',
  version: '0.1.0',
  platforms: ['ios'],
  orientation: 'default',
  userInterfaceStyle: 'automatic',
  icon: './assets/icon.png',
  backgroundColor: tokens.color.dark.background,
  // English is the source for the system permission prompts; other languages override it here.
  locales: {
    en: './locales/en.json',
    tr: './locales/tr.json',
  },
  ios: {
    bundleIdentifier: BUNDLE_IDENTIFIER,
    supportsTablet: false,
    requireFullScreen: false,
    config: {
      // The app only uses the encryption built into iOS (CryptoKit) to protect the local stream,
      // which is exempt from export documentation. The owner should confirm this declaration.
      usesNonExemptEncryption: false,
    },
    infoPlist: {
      NSCameraUsageDescription:
        'PairCam uses the camera to send live video to your computer, where it appears as a webcam.',
      NSLocalNetworkUsageDescription:
        'PairCam uses your local network so the PairCam app on your computer can find this iPhone and receive its video.',
      NSBonjourServices: [BONJOUR_SERVICE],
      UISupportedInterfaceOrientations: [
        'UIInterfaceOrientationPortrait',
        'UIInterfaceOrientationLandscapeLeft',
        'UIInterfaceOrientationLandscapeRight',
      ],
      // Lets animations run at 120 Hz on ProMotion iPhones.
      CADisableMinimumFrameDurationOnPhone: true,
    },
  },
  plugins: [
    'expo-router',
    'expo-dev-client',
    'expo-sqlite',
    [
      'expo-localization',
      {
        supportedLocales: ['en', 'tr'],
      },
    ],
    [
      'expo-splash-screen',
      {
        image: './assets/splash-icon.png',
        imageWidth: 96,
        resizeMode: 'contain',
        backgroundColor: tokens.color.light.background,
        dark: {
          image: './assets/splash-icon-dark.png',
          backgroundColor: tokens.color.dark.background,
        },
      },
    ],
    [
      'expo-build-properties',
      {
        ios: {
          deploymentTarget: '16.4',
        },
      },
    ],
  ],
  experiments: {
    typedRoutes: true,
  },
  extra: EAS_PROJECT_ID ? { eas: { projectId: EAS_PROJECT_ID } } : undefined,
});
