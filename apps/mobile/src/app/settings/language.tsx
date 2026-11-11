import * as React from 'react';
import { useTranslation } from 'react-i18next';

import { ListRow, ListSection } from '@/components/ui/list';
import { ScreenScroll } from '@/components/ui/screen-scroll';
import { LANGUAGES } from '@/i18n';
import { haptics } from '@/lib/haptics';
import { setLanguage, usePreferences } from '@/lib/preferences';

export default function LanguageSettings() {
  const { t } = useTranslation();
  const { language } = usePreferences();

  const choose = (choice: typeof language) => {
    if (choice !== language) {
      haptics.selection();
      setLanguage(choice);
    }
  };

  return (
    <ScreenScroll>
      <ListSection footer={t('settings.language.footer')}>
        <ListRow
          title={t('settings.language.system')}
          selected={language === null}
          onPress={() => choose(null)}
        />
        {LANGUAGES.map((code) => (
          // Each language is listed under its own name, so it can be found from any language.
          <ListRow
            key={code}
            title={t(`languages.${code}`)}
            selected={language === code}
            onPress={() => choose(code)}
          />
        ))}
      </ListSection>
    </ScreenScroll>
  );
}
