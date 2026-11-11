import type { PairedHost } from 'camera-engine';
import Constants from 'expo-constants';
import { useRouter } from 'expo-router';
import * as React from 'react';
import { useTranslation } from 'react-i18next';
import { Alert, View } from 'react-native';

import { Button } from '@/components/ui/button';
import { ListRow, ListSection } from '@/components/ui/list';
import { ScreenScroll } from '@/components/ui/screen-scroll';
import { Switch } from '@/components/ui/switch';
import { Text } from '@/components/ui/text';
import { engine, useEngine } from '@/engine/store';
import { mediumDate } from '@/lib/format';
import { haptics } from '@/lib/haptics';
import { setFaceTracking, setGestures, usePreferences } from '@/lib/preferences';

/** The gestures explained in Settings, in the order they are listed. */
const GESTURES = ['palm', 'frame', 'spread'] as const;

/** Explanations of the gestures may wrap; they are the whole manual. */
const GESTURE_BODY_LINES = 3;
const HINT_LINES = 2;

export default function Settings() {
  const { t, i18n } = useTranslation();
  const router = useRouter();
  const { language, appearance, faceTracking, gestures } = usePreferences();
  const [hosts, setHosts] = React.useState<PairedHost[] | null>(null);

  // The list changes when a computer pairs or a session ends, so it is read again whenever the
  // engine's status changes.
  const status = useEngine((snapshot) => snapshot.state.status);
  React.useEffect(() => {
    let current = true;
    engine
      .pairedHosts()
      .then((loaded) => {
        if (current) {
          setHosts(loaded);
        }
      })
      .catch(() => {
        if (current) {
          setHosts([]);
        }
      });
    return () => {
      current = false;
    };
  }, [status]);

  const forget = (host: PairedHost) => {
    Alert.alert(
      t('settings.pairedComputers.forgetTitle', { host: host.name }),
      t('settings.pairedComputers.forgetBody'),
      [
        { text: t('common.cancel'), style: 'cancel' },
        {
          text: t('settings.pairedComputers.forget'),
          style: 'destructive',
          onPress: () => {
            haptics.medium();
            setHosts((list) => list?.filter((item) => item.id !== host.id) ?? null);
            engine.forgetHost(host.id).catch(() => {
              // The row comes back the next time the list is read.
            });
          },
        },
      ],
    );
  };

  return (
    <ScreenScroll>
      <ListSection
        title={t('settings.pairedComputers.title')}
        footer={t('settings.pairedComputers.footer')}
      >
        {hosts === null ? null : hosts.length === 0 ? (
          <View className="px-4 py-5">
            <Text tone="muted">{t('settings.pairedComputers.empty')}</Text>
          </View>
        ) : (
          hosts.map((host) => (
            <ListRow
              key={host.id}
              title={host.name}
              subtitle={
                host.pairedAt === undefined
                  ? undefined
                  : t('settings.pairedComputers.pairedOn', {
                      date: mediumDate(host.pairedAt, i18n.language),
                    })
              }
              trailing={
                <Button
                  variant="ghost"
                  size="md"
                  label={t('settings.pairedComputers.forget')}
                  accessibilityLabel={t('settings.pairedComputers.forgetLabel', {
                    host: host.name,
                  })}
                  onPress={() => forget(host)}
                />
              }
            />
          ))
        )}
      </ListSection>

      <ListSection title={t('settings.handsFree.title')} footer={t('settings.handsFree.footer')}>
        <ListRow
          title={t('settings.handsFree.faceTracking')}
          subtitle={t('settings.handsFree.faceTrackingHint')}
          subtitleLines={HINT_LINES}
          trailing={
            <Switch
              value={faceTracking}
              onValueChange={setFaceTracking}
              accessibilityLabel={t('settings.handsFree.faceTracking')}
            />
          }
        />
        <ListRow
          title={t('settings.handsFree.gestures')}
          subtitle={t('settings.handsFree.gesturesHint')}
          subtitleLines={HINT_LINES}
          trailing={
            <Switch
              value={gestures}
              onValueChange={setGestures}
              accessibilityLabel={t('settings.handsFree.gestures')}
            />
          }
        />
      </ListSection>

      <ListSection title={t('settings.gestures.title')}>
        {GESTURES.map((kind) => (
          <ListRow
            key={kind}
            title={t(`settings.gestures.${kind}.title`)}
            subtitle={t(`settings.gestures.${kind}.body`)}
            subtitleLines={GESTURE_BODY_LINES}
          />
        ))}
      </ListSection>

      <ListSection title={t('settings.preferences.title')}>
        <ListRow
          title={t('settings.language.title')}
          value={language ? t(`languages.${language}`) : t('settings.language.system')}
          navigates
          onPress={() => router.push('/settings/language')}
        />
        <ListRow
          title={t('settings.appearance.title')}
          value={t(`settings.appearance.${appearance}`)}
          navigates
          onPress={() => router.push('/settings/appearance')}
        />
      </ListSection>

      <ListSection title={t('settings.about.title')} footer={t('settings.about.footer')}>
        <ListRow
          title={t('settings.about.version')}
          value={Constants.expoConfig?.version ?? ''}
          monoValue
        />
        <ListRow
          title={t('settings.about.licences')}
          navigates
          onPress={() => router.push('/settings/licences')}
        />
      </ListSection>
    </ScreenScroll>
  );
}
