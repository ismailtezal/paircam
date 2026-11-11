import * as React from 'react';
import { useTranslation } from 'react-i18next';

import { ListRow, ListSection } from '@/components/ui/list';
import { ScreenScroll } from '@/components/ui/screen-scroll';
import { haptics } from '@/lib/haptics';
import { APPEARANCES, setAppearance, usePreferences } from '@/lib/preferences';

export default function AppearanceSettings() {
  const { t } = useTranslation();
  const { appearance } = usePreferences();

  return (
    <ScreenScroll>
      <ListSection>
        {APPEARANCES.map((choice) => (
          <ListRow
            key={choice}
            title={t(`settings.appearance.${choice}`)}
            selected={appearance === choice}
            onPress={() => {
              if (choice !== appearance) {
                haptics.selection();
                setAppearance(choice);
              }
            }}
          />
        ))}
      </ListSection>
    </ScreenScroll>
  );
}
