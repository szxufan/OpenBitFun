import { Button, Select } from '@openbitfun/ui';
import { useI18n } from '@/infrastructure/i18n';
import { ConfigPageRow } from './common';
import type { ResponsesRequestSettings } from '../types';

export function ResponsesRequestSettingsFields({ value, onChange, onReset, supported = true }: {
  value?: ResponsesRequestSettings;
  onChange: (value: ResponsesRequestSettings) => void;
  supported?: boolean;
  onReset?: () => void;
}) {
  const { t } = useI18n('settings/models');
  const transport = value?.transport ?? 'http';
  return <>
    <ConfigPageRow label={t('advancedSettings.responses.transportLabel')}
      description={t(supported ? 'advancedSettings.responses.transportHint' : 'advancedSettings.responses.unsupported')}>
      <Select size="sm" value={transport} disabled={!supported}
        aria-label={t('advancedSettings.responses.transportLabel')}
        options={[
          { value: 'http', label: 'HTTP' },
          { value: 'websocket', label: 'WebSocket' },
        ]}
        onValueChange={selected => onChange({
          transport: selected === 'websocket' ? 'websocket' : 'http',
          context: selected === 'http' ? 'full' : value?.context ?? 'full',
        })} />
    </ConfigPageRow>
    <ConfigPageRow label={t('advancedSettings.responses.contextLabel')}
      description={t('advancedSettings.responses.contextHint')}>
      <Select size="sm" value={value?.context ?? 'full'} disabled={!supported || transport === 'http'}
        aria-label={t('advancedSettings.responses.contextLabel')}
        options={[
          { value: 'full', label: t('advancedSettings.responses.full') },
          { value: 'incremental', label: t('advancedSettings.responses.incremental') },
        ]}
        onValueChange={selected => onChange({ transport,
          context: selected === 'incremental' ? 'incremental' : 'full' })} />
    </ConfigPageRow>
    {onReset && <Button size="sm" disabled={!supported} onClick={onReset}>{t('advancedSettings.responses.reset')}</Button>}
  </>;
}
