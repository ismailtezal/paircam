import * as Brightness from 'expo-brightness';
import * as React from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet } from 'react-native';
import Animated, { FadeIn, FadeOut } from 'react-native-reanimated';

import { Text } from '@/components/ui/text';
import { haptics } from '@/lib/haptics';
import { motion, tokens } from '@/theme';

/** How long the reminder stays before the screen goes fully black. */
const HINT_VISIBLE_MS = 4000;

/** The backlight level while dimmed. Black pixels alone do not save power on LCD iPhones. */
const DIMMED_BRIGHTNESS = 0.01;

// The dimmed screen is black in both themes; that is the point of it.
const BLACK = tokens.color.dark.background;
const HINT_COLOR = tokens.color.dark.textSubtle;

const ignore = () => {
  // Brightness control is a saving, not a requirement.
};

/**
 * Blacks the screen while the stream carries on, to save battery and heat during a long call.
 * iOS stops the camera if the phone is locked, so this is the way to "turn the screen off".
 *
 * Any tap wakes it. A line says so for a few seconds and then fades, leaving the screen fully
 * black.
 */
export function DimOverlay({ onWake }: { onWake: () => void }) {
  const { t } = useTranslation();
  const [hintVisible, setHintVisible] = React.useState(true);

  React.useEffect(() => {
    const timer = setTimeout(() => setHintVisible(false), HINT_VISIBLE_MS);
    return () => clearTimeout(timer);
  }, []);

  React.useEffect(() => {
    let previous: number | null = null;
    let woken = false;

    Brightness.getBrightnessAsync()
      .then((current) => {
        if (woken) {
          return;
        }
        previous = current;
        return Brightness.setBrightnessAsync(DIMMED_BRIGHTNESS);
      })
      .catch(ignore);

    return () => {
      woken = true;
      if (previous !== null) {
        Brightness.setBrightnessAsync(previous).catch(ignore);
      }
    };
  }, []);

  return (
    <Animated.View
      style={[StyleSheet.absoluteFill, styles.black]}
      entering={FadeIn.duration(motion.duration.slow)}
      exiting={FadeOut.duration(motion.duration.fast)}
    >
      <Pressable
        style={styles.target}
        accessibilityRole="button"
        accessibilityLabel={t('streaming.wake')}
        onPress={() => {
          haptics.light();
          onWake();
        }}
      >
        {hintVisible ? (
          <Animated.View exiting={FadeOut.duration(motion.duration.slow)}>
            <Text variant="label" style={styles.hint}>
              {t('streaming.dimmed')}
            </Text>
          </Animated.View>
        ) : null}
      </Pressable>
    </Animated.View>
  );
}

const styles = StyleSheet.create({
  black: {
    backgroundColor: BLACK,
  },
  target: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: tokens.space[8],
  },
  hint: {
    color: HINT_COLOR,
    textAlign: 'center',
  },
});
