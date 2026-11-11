import { Stack, useRouter } from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import * as React from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable } from 'react-native';

import { Text } from '@/components/ui/text';
import { useEngine } from '@/engine/store';
import { tokens, tracking, useTheme } from '@/theme';

export default function SettingsLayout() {
  const { t } = useTranslation();
  const router = useRouter();
  const { colors, mode } = useTheme();
  const status = useEngine((snapshot) => snapshot.state.status);

  // A pairing request is answered on the camera screen, so Settings gets out of its way.
  React.useEffect(() => {
    if (status === 'pairing') {
      router.dismissTo('/');
    }
  }, [router, status]);

  return (
    <>
      <StatusBar style={mode === 'dark' ? 'light' : 'dark'} />
      <Stack
        screenOptions={{
          headerStyle: { backgroundColor: colors.background },
          headerShadowVisible: false,
          headerTintColor: colors.text,
          headerTitleStyle: {
            color: colors.text,
            fontSize: tokens.font.size.md,
            fontWeight: tokens.font.weight.semibold,
          },
          headerBackButtonDisplayMode: 'minimal',
          contentStyle: { backgroundColor: colors.background },
        }}
      >
        <Stack.Screen
          name="index"
          options={{
            title: t('settings.title'),
            headerRight: () => (
              <Pressable
                onPress={() => router.dismissTo('/')}
                hitSlop={tokens.space[3]}
                accessibilityRole="button"
              >
                <Text
                  variant="headline"
                  tone="accent"
                  style={{ letterSpacing: tracking(tokens.font.size.md, 'normal') }}
                >
                  {t('common.done')}
                </Text>
              </Pressable>
            ),
          }}
        />
        <Stack.Screen name="language" options={{ title: t('settings.language.title') }} />
        <Stack.Screen name="appearance" options={{ title: t('settings.appearance.title') }} />
        <Stack.Screen name="licences" options={{ title: t('settings.licences.title') }} />
      </Stack>
    </>
  );
}
