import { useEffect, useRef, useState } from 'react';
import { Button, Input, MultiSelect, Switch } from '@openbitfun/ui';
import { ConfigPageRow, ConfigPageSection } from '@/infrastructure/config/components/common';
import { desktopControllerSettings, type ControllerRequest, type DesktopControllerSettings } from '@/infrastructure/api/service-api/DesktopExternalControlAPI';
import { useI18n } from '@/infrastructure/i18n';

export default function ExternalControlSection() {
  const { t } = useI18n('settings/runtime');
  const [settings, setSettings] = useState<DesktopControllerSettings>();
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [port, setPort] = useState('');
  const [name, setName] = useState('');
  const [workspaceIds, setWorkspaceIds] = useState<string[]>([]);
  const [origin, setOrigin] = useState('');
  const activeRequest = useRef<symbol | null>(null);

  async function run(request: ControllerRequest) {
    const requestId = Symbol();
    activeRequest.current = requestId;
    setBusy(true);
    setError('');
    try {
      const result = await desktopControllerSettings(request);
      if (activeRequest.current !== requestId) return;
      setSettings(result);
      setPort(String(result.port));
      if (request.operation === 'issue') {
        setName('');
        setWorkspaceIds([]);
        setOrigin('');
      }
    } catch (err) {
      if (activeRequest.current === requestId) setError(String(err));
    } finally {
      if (activeRequest.current === requestId) setBusy(false);
    }
  }

  useEffect(() => {
    void run({ operation: 'status' });
    return () => { activeRequest.current = null; };
  }, []);

  return <ConfigPageSection title={t('externalControl.title')}>
    <ConfigPageRow label={t('externalControl.enable')} description={t('externalControl.description')}>
      <Switch aria-label={t('externalControl.enable')} checked={settings?.enabled ?? false}
        disabled={!settings || busy || Number(port) < 1 || Number(port) > 65535 || !Number.isInteger(Number(port))} onChange={event => void run({ operation: 'setEnabled', enabled: event.target.checked, port: Number(port) })} />
    </ConfigPageRow>
    {settings && !settings.listening && <ConfigPageRow label={t('externalControl.port')}>
      <Input size="sm" type="number" min={1} max={65535} aria-label={t('externalControl.port')} value={port} onChange={e => setPort(e.target.value)} />
    </ConfigPageRow>}
    {(error || settings?.error) && <ConfigPageRow label={error || settings?.error}>
      <Button size="sm" disabled={busy} onClick={() => void run(settings?.enabled && !settings.listening
        ? { operation: 'setEnabled', enabled: true, port: Number(port) } : { operation: 'status' })}>{t('externalControl.retry')}</Button>
    </ConfigPageRow>}
    {settings?.listening && <>
      <ConfigPageRow label={t('externalControl.endpoint')}><code>{settings.endpoint}</code></ConfigPageRow>
      <ConfigPageRow label={t('externalControl.name')}>
        <Input size="sm" aria-label={t('externalControl.name')} maxLength={128} value={name} onChange={e => setName(e.target.value)} />
      </ConfigPageRow>
      <ConfigPageRow label={t('externalControl.workspaces')}>
        <MultiSelect size="sm" showSelectAll aria-label={t('externalControl.workspaces')} value={workspaceIds}
          options={settings.workspaces.map(w => ({ value: w.id, label: w.name }))} onValueChange={ids => setWorkspaceIds(ids.map(String))} />
      </ConfigPageRow>
      <ConfigPageRow label={t('externalControl.origin')} description={t('externalControl.originHint')}>
        <Input size="sm" aria-label={t('externalControl.origin')} value={origin} onChange={e => setOrigin(e.target.value)} />
      </ConfigPageRow>
      <ConfigPageRow label={t('externalControl.access')}>
        <Button variant="primary" size="sm" disabled={busy || !name.trim() || !workspaceIds.length}
          onClick={() => void run({ operation: 'issue', name, workspaceIds, origin: origin.trim() || undefined })}>{t('externalControl.issue')}</Button>
      </ConfigPageRow>
      {settings.token && <ConfigPageRow label={t('externalControl.token')} description={t('externalControl.tokenHint')}>
        <Input size="sm" aria-label={t('externalControl.token')} value={settings.token} readOnly onFocus={e => e.target.select()} />
      </ConfigPageRow>}
    </>}
    {settings?.grants.map(grant => <ConfigPageRow key={grant.id} label={grant.name}
      description={grant.workspaceIds.map(id => settings.workspaces.find(w => w.id === id)?.name ?? id).join(', ')}>
      <Button size="sm" disabled={busy} onClick={() => void run({ operation: 'revoke', id: grant.id })}>{t('externalControl.revoke')}</Button>
    </ConfigPageRow>)}
  </ConfigPageSection>;
}
