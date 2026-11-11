import * as React from 'react';
import { useTranslation } from 'react-i18next';

import { ListRow, ListSection } from '@/components/ui/list';
import { ScreenScroll } from '@/components/ui/screen-scroll';
import { Text } from '@/components/ui/text';
import licences from '@/generated/licences.json';

export default function Licences() {
  const { t } = useTranslation();

  return (
    <ScreenScroll>
      <Text tone="muted" className="px-4">
        {t('settings.licences.intro')}
      </Text>
      <ListSection>
        {licences.map((entry) => (
          <ListRow
            key={entry.name}
            title={entry.name}
            subtitle={entry.version}
            value={entry.licence ?? t('settings.licences.unknown')}
          />
        ))}
      </ListSection>
    </ScreenScroll>
  );
}
