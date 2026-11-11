import * as React from 'react';
import { ScrollView, StyleSheet } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { tokens } from '@/theme';

import { SHEET_MAX_WIDTH } from './sheet';

/**
 * The scrolling body of a settings-style screen: a single centred column on the app background
 * that stays readable in landscape and clears the home indicator.
 */
function ScreenScroll({ children }: { children: React.ReactNode }) {
  const insets = useSafeAreaInsets();
  return (
    <ScrollView
      className="flex-1 bg-background"
      contentInsetAdjustmentBehavior="automatic"
      contentContainerStyle={[
        styles.content,
        {
          paddingBottom: insets.bottom + tokens.space[8],
          paddingLeft: Math.max(insets.left, tokens.space[4]),
          paddingRight: Math.max(insets.right, tokens.space[4]),
        },
      ]}
    >
      {children}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  content: {
    width: '100%',
    maxWidth: SHEET_MAX_WIDTH + tokens.space[4] * 2,
    alignSelf: 'center',
    paddingTop: tokens.space[4],
    gap: tokens.space[8],
  },
});

export { ScreenScroll };
