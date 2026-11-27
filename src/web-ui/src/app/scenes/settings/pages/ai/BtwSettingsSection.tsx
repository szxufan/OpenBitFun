import { useState } from 'react';
import { Switch } from '@openbitfun/ui';
import { useI18n } from '@/infrastructure/i18n';
import { ConfigPageRow, ConfigPageSection } from '@/infrastructure/config/components/common';
import { setConfirmBtwClose, shouldConfirmBtwClose } from '@/infrastructure/preferences/btwCloseConfirmation';
import { useSettingsSectionAnchor } from '../shared/useSettingsSectionAnchor';
import { BtwModelSettingsRow } from './BtwModelSettingsRow';

export default function BtwSettingsSection() {
  const { t } = useI18n('settings/application');
  const { t: tModels } = useI18n('settings/models');
  const [enabled, setEnabled] = useState(shouldConfirmBtwClose);
  const sectionAnchor = useSettingsSectionAnchor('btw', true);

  return (
    <div id={sectionAnchor}>
      <ConfigPageSection title={tModels('btwModel.title')}>
        <BtwModelSettingsRow />
        <ConfigPageRow label={t('btwCloseConfirmation.label')} description={t('btwCloseConfirmation.description')} align="center">
          <Switch
            checked={enabled}
            onCheckedChange={(next) => {
              setEnabled(next);
              setConfirmBtwClose(next);
            }}
            aria-label={t('btwCloseConfirmation.label')}
          />
        </ConfigPageRow>
      </ConfigPageSection>
    </div>
  );
}
