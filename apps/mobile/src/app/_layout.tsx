import '../../global.css';
import '@/i18n';

import { Stack } from 'expo-router';
import * as React from 'react';
import { StyleSheet } from 'react-native';
import { GestureHandlerRootView } from 'react-native-gesture-handler';

import { applyStoredAppearance, applyStoredCameraControls } from '@/lib/preferences';
import { useTheme } from '@/theme';

// Applied before the first render so the app never flashes the wrong theme.
applyStoredAppearance();
// The engine keeps no settings of its own; it learns them before any computer can connect.
applyStoredCameraControls();

export default function RootLayout() {
  const { colors } = useTheme();

  return (
    <GestureHandlerRootView style={styles.root}>
      <Stack
        screenOptions={{
          headerShown: false,
          contentStyle: { backgroundColor: colors.background },
        }}
      >
        <Stack.Screen name="index" />
        {/* Settings is a place the user goes and comes back from, so it is the platform's own
            sheet, with its own drag-to-dismiss, rather than something built here. */}
        <Stack.Screen name="settings" options={{ presentation: 'modal' }} />
      </Stack>
    </GestureHandlerRootView>
  );
}

const styles = StyleSheet.create({
  root: {
    flex: 1,
  },
});
