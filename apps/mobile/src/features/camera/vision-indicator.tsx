import type { GestureKind } from 'camera-engine';
import * as React from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo } from 'react-native';
import Animated, { FadeIn, FadeInUp, FadeOut, FadeOutUp } from 'react-native-reanimated';

import { Icon, type IconName } from '@/components/ui/icon';
import { Material } from '@/components/ui/material';
import { Text } from '@/components/ui/text';
import { useEngine } from '@/engine/store';
import { haptics } from '@/lib/haptics';
import { motion } from '@/theme';

/** How long a recognised gesture stays on screen: long enough to read at a glance. */
const GESTURE_VISIBLE_MS = 1500;

const GESTURE_ICON: Record<GestureKind, IconName> = {
  palm: 'hand.raised',
  frame: 'plus.magnifyingglass',
  spread: 'arrow.up.left.and.arrow.down.right',
};

type CapsuleProps = {
  icon: IconName;
  label: string;
  accessibilityLabel: string;
};

/** A small translucent capsule over the preview: an icon and a word or two. */
function Capsule({ icon, label, accessibilityLabel }: CapsuleProps) {
  return (
    <Material
      className="h-touch-target flex-row items-center gap-2 rounded-full px-4"
      accessible
      accessibilityRole="text"
      accessibilityLabel={accessibilityLabel}
    >
      <Icon name={icon} size="sm" />
      <Text variant="label" numberOfLines={1}>
        {label}
      </Text>
    </Material>
  );
}

/**
 * Sits next to the status while face tracking is on, so the user knows the computer is framing
 * them. It appears and leaves with a short fade; the change itself comes from a gesture (which
 * has its own haptic), the computer or Settings.
 */
export function TrackingBadge() {
  const { t } = useTranslation();
  const faceTracking = useEngine((snapshot) => snapshot.environment.faceTracking);
  if (!faceTracking) {
    return null;
  }
  return (
    <Animated.View
      entering={FadeIn.duration(motion.duration.base).easing(motion.easing.out)}
      exiting={FadeOut.duration(motion.duration.fast).easing(motion.easing.out)}
    >
      <Capsule
        icon="person.fill.viewfinder"
        label={t('vision.tracking')}
        accessibilityLabel={t('vision.trackingLabel')}
      />
    </Animated.View>
  );
}

/**
 * Names the gesture the camera just acted on, with a light haptic, so a user who is not looking
 * at the computer still knows it worked. A one-off gesture shows briefly; a spread stays up while
 * the hands move and leaves shortly after they stop.
 */
export function GestureToast() {
  const { t } = useTranslation();
  const gesture = useEngine((snapshot) => snapshot.gesture);
  const [dismissed, setDismissed] = React.useState<number | null>(null);
  const announced = React.useRef<number | null>(null);

  // Feedback once per gesture, even if the component re-renders or the language changes.
  React.useEffect(() => {
    if (!gesture || gesture.phase === 'ended' || announced.current === gesture.id) {
      return;
    }
    announced.current = gesture.id;
    haptics.light();
    AccessibilityInfo.announceForAccessibility(
      t('vision.gestureAnnouncement', { gesture: t(`vision.gesture.${gesture.kind}`) }),
    );
  }, [gesture, t]);

  React.useEffect(() => {
    if (!gesture || gesture.phase === 'began') {
      return;
    }
    const timer = setTimeout(() => setDismissed(gesture.id), GESTURE_VISIBLE_MS);
    return () => clearTimeout(timer);
  }, [gesture]);

  const shown = gesture && gesture.id !== dismissed ? gesture : null;
  if (!shown) {
    return null;
  }
  const label = t(`vision.gesture.${shown.kind}`);
  // Keyed by kind, so a spread that ends keeps its capsule instead of replaying the entrance.
  return (
    <Animated.View
      key={shown.kind}
      entering={FadeInUp.duration(motion.duration.base).easing(motion.easing.out)}
      exiting={FadeOutUp.duration(motion.duration.fast).easing(motion.easing.out)}
    >
      <Capsule
        icon={GESTURE_ICON[shown.kind]}
        label={label}
        accessibilityLabel={t('vision.gestureAnnouncement', { gesture: label })}
      />
    </Animated.View>
  );
}
