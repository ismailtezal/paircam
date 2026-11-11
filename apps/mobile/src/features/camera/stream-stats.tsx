import type { EngineState, StreamStats as Stats, ThermalState } from 'camera-engine';
import * as React from 'react';
import { useTranslation } from 'react-i18next';
import { View } from 'react-native';

import { Stat } from '@/components/ui/stat';
import type { TextProps } from '@/components/ui/text';
import { useEngine } from '@/engine/store';
import { megabits, percentNumber, whole } from '@/lib/format';

type StreamingState = Extract<EngineState, { status: 'streaming' }>;

const THERMAL_TONE: Record<ThermalState, TextProps['tone']> = {
  nominal: 'default',
  fair: 'default',
  serious: 'warning',
  critical: 'danger',
  unknown: 'muted',
};

/**
 * The live numbers, always on screen while streaming: what is being sent (size, frame rate,
 * bitrate) and how the phone is coping (temperature, battery).
 *
 * This is the only component that re-renders every second; it reads the stats itself so the rest
 * of the screen does not.
 */
export function StreamStats({ stream }: { stream: StreamingState }) {
  const { t, i18n } = useTranslation();
  const stats: Stats | null = useEngine((snapshot) => snapshot.stats);
  const language = i18n.language;
  const pending = t('streaming.pending');

  const battery = stats
    ? t(stats.charging ? 'streaming.batteryCharging' : 'streaming.batteryValue', {
        value: percentNumber(stats.battery, language),
      })
    : pending;

  return (
    <View className="gap-3">
      <View className="flex-row gap-4">
        <Stat
          className="flex-1"
          label={t('streaming.resolution')}
          value={t('streaming.resolutionValue', { width: stream.width, height: stream.height })}
        />
        <Stat
          className="flex-1"
          label={t('streaming.frameRate')}
          value={
            stats ? t('streaming.frameRateValue', { value: whole(stats.fps, language) }) : pending
          }
        />
        <Stat
          className="flex-1"
          label={t('streaming.bitrate')}
          value={
            stats
              ? t('streaming.bitrateValue', { value: megabits(stats.bitrate, language) })
              : pending
          }
        />
      </View>
      <View className="flex-row gap-4">
        <Stat
          className="flex-1"
          label={t('streaming.temperature')}
          value={stats ? t(`streaming.thermal.${stats.thermal}`) : pending}
          tone={stats ? THERMAL_TONE[stats.thermal] : 'default'}
        />
        <Stat className="flex-1" label={t('streaming.battery')} value={battery} />
        <View className="flex-1" />
      </View>
    </View>
  );
}
