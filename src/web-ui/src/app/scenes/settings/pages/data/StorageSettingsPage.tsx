import { useEffect, useRef, useState } from 'react';
import { Folder, Trash2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Button, Disclosure, OverflowText, Select, captureConfirmationAnchor } from '@openbitfun/ui';
import { snapshotStorageAPI, type StorageOverview, type StoragePeriod, type WorkspaceUsage } from '@/infrastructure/api/service-api/SnapshotStorageAPI';
import { ConfigLoadingState, ConfigMessage, ConfigPageSection, ConfigRefreshButton } from '@/infrastructure/config/components/common';
import { confirmDanger } from '@/infrastructure/confirm-dialog';
import { i18nService } from '@/infrastructure/i18n';
import { isTauriRuntime } from '@/infrastructure/runtime';
import type { SettingsPageProps } from '../../settingsTypes';
import { SettingsPage } from '../shared/SettingsPage';
import styles from './StorageSettingsPage.module.scss';

function size(bytes: number) {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
  const index = Math.min(units.length - 1, Math.floor(Math.log2(Math.max(1, bytes)) / 10));
  return `${i18nService.formatNumber(bytes / 1024 ** index, { maximumFractionDigits: 1 })} ${units[index]}`;
}

function WorkspaceStorage({ data }: { data: StorageOverview }) {
  const { t } = useTranslation('settings');
  const usage = (bytes: number) => (
    <div className={styles.usage}>
      <strong>{size(bytes)}</strong>
      <span aria-hidden="true" className={styles.track}>
        <span style={{ width: `${data.totalBytes ? Math.min(100, bytes / data.totalBytes * 100) : 0}%` }} />
      </span>
    </div>
  );

  return (
    <div className={styles.panel} data-openbitfun-component="snapshot-storage" data-openbitfun-part="panel">
      <div className={styles.total} data-openbitfun-component="snapshot-storage" data-openbitfun-part="total">
        <span>{t('storage.total')}</span>
        <strong>{size(data.totalBytes)}</strong>
      </div>
      {(data.workspaces.length > 0 || data.unidentifiedBytes > 0) && (
        <div className={styles.workspaceScroll} data-openbitfun-component="snapshot-storage" data-openbitfun-part="workspaceList" role="region" aria-label={t('shared:features.workspace')} tabIndex={0}>
          <table className={styles.table} aria-label={t('shared:features.workspace')}>
            <thead>
              <tr>
                <th scope="col">{t('shared:features.workspace')}</th>
                <th scope="col">{t('storage.sharedColumn')}</th>
                <th scope="col">{t('storage.usedColumn')}</th>
              </tr>
            </thead>
            <tbody>
              {data.workspaces.map(row => (
                <tr key={row.workspaceId}>
                  <th scope="row">
                    <OverflowText className={styles.workspaceName}>{row.name}</OverflowText>
                    <OverflowText className={styles.location} title={row.location}>{row.location}</OverflowText>
                  </th>
                  <td>{row.warning ? '—' : size(row.sharedBytes)}</td>
                  <td>{usage(row.bytes)}</td>
                </tr>
              ))}
              {data.unidentifiedBytes > 0 && (
                <tr>
                  <th scope="row">
                    <span className={styles.workspaceName}>{t('storage.unidentified')}</span>
                    <span className={styles.location}>{t('storage.unidentifiedHint')}</span>
                  </th>
                  <td>—</td>
                  <td>{usage(data.unidentifiedBytes)}</td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function SessionRows({ workspace }: { workspace: WorkspaceUsage }) {
  const { t } = useTranslation('settings');
  return (
    <ul className={styles.sessions}>
      {workspace.candidates.map(session => (
        <li key={session.sessionId} className={styles.sessionRow} data-openbitfun-component="snapshot-storage" data-openbitfun-part="session">
          <span className={styles.sessionTitle}>{session.name || session.sessionId}</span>
          {session.removed ? <span className={styles.location}>{t('storage.removedSession')}</span> : <time dateTime={new Date(session.activeAt).toISOString()}>
            {i18nService.formatDate(new Date(session.activeAt), { dateStyle: 'medium' })}
          </time>}
        </li>
      ))}
    </ul>
  );
}

function CandidateList({ rows }: { rows: WorkspaceUsage[] }) {
  const { t } = useTranslation('settings');
  return (
    <div className={styles.candidateScroll} data-openbitfun-component="snapshot-storage" data-openbitfun-part="candidateList" role="region" aria-label={t('storage.candidates')} tabIndex={0}>
      <div className={styles.listHeader}>
        <span>{t('storage.candidates')}</span>
        <span>{t('storage.lastActive')}</span>
      </div>
      {rows.filter(row => row.candidates.length > 0).map(row => (
        <Disclosure key={row.workspaceId} unmountOnClose
          summary={row.name} leading={<Folder size={15} />}
          triggerClassName={styles.groupTrigger} contentInnerClassName={styles.groupBody}
          actions={<span className={styles.groupCount}>
            {t('storage.sessionCount', { sessions: i18nService.formatNumber(row.candidates.length) })}
          </span>}>
          <SessionRows workspace={row} />
        </Disclosure>
      ))}
    </div>
  );
}

function FileSnapshotStorageSection({ isActive }: { isActive: boolean }) {
  const { t } = useTranslation('settings');
  const [period, setPeriod] = useState<StoragePeriod>('thirtyDays');
  const [workspace, setWorkspace] = useState('');
  const [refresh, setRefresh] = useState(0);
  const [data, setData] = useState<StorageOverview>();
  const [loading, setLoading] = useState(true);
  const [cleaning, setCleaning] = useState(false);
  const [error, setError] = useState('');
  const [outcome, setOutcome] = useState<{ type: 'success' | 'warning'; text: string } | null>(null);
  const inProgress = useRef(false);
  const supported = isTauriRuntime();

  useEffect(() => {
    if (!isActive || !supported) return;
    let current = true;
    setLoading(true);
    setError('');
    snapshotStorageAPI.inspect(period).then(result => {
      if (current) setData(result);
    }).catch(e => {
      if (current) { setError(String(e)); setData(undefined); }
    }).finally(() => { if (current) setLoading(false); });
    return () => { current = false; };
  }, [period, refresh, isActive, supported]);

  const rows = (data?.workspaces ?? []).filter(row => !workspace || workspace === row.workspaceId);
  const count = rows.reduce((n, row) => n + row.candidates.length, 0);
  const estimated = rows.reduce((n, row) => n + row.estimatedBytes, 0);
  const skipped = rows.reduce((n, row) => n + row.skippedSessions, 0);
  const busy = loading || cleaning;
  const preview = t('storage.preview', { sessions: i18nService.formatNumber(count), size: size(estimated) });

  async function clean(trigger: HTMLElement) {
    if (inProgress.current || busy || !count) return;
    inProgress.current = true;
    setCleaning(true);
    try {
      if (!await confirmDanger(t('storage.confirmTitle'), t('storage.confirm', { preview }), {
        anchor: captureConfirmationAnchor(trigger), confirmText: t('storage.cleanup'),
      })) return;
      let bytes = 0;
      let removed = 0;
      let missed = 0;
      const errors: string[] = [];
      for (const row of rows) {
        if (!row.candidates.length) continue;
        try {
          const result = await snapshotStorageAPI.clean(row.workspaceId, period, row.candidates);
          bytes += result.freedBytes;
          removed += result.removedSessions;
          missed += result.skippedSessions;
          errors.push(...result.errors);
        } catch (e) {
          missed += row.candidates.length;
          errors.push(`${row.name} · ${String(e)}`);
        }
      }
      const summary = t('storage.result', { sessions: i18nService.formatNumber(removed), skipped: i18nService.formatNumber(missed), size: size(bytes) });
      setOutcome({ type: errors.length || missed ? 'warning' : 'success', text: [summary, ...errors].join('\n') });
      setLoading(true);
      setRefresh(v => v + 1);
    } finally {
      setCleaning(false);
      inProgress.current = false;
    }
  }

  if (!supported) return <ConfigMessage message={{ type: 'info', text: t('storage.desktopOnly') }} />;

  return (
    <>
      <ConfigPageSection title={t('storage.snapshots')} bodySurface={false}
        description={t('storage.device')}
        extra={<ConfigRefreshButton tooltip={t('storage.refresh')} disabled={busy} loading={loading}
          onClick={() => { setLoading(true); setRefresh(v => v + 1); }} />}>
        {loading && !data && <ConfigLoadingState label={t('storage.scanning')} variant="list" />}
        {error && <ConfigMessage message={{ type: 'error', text: error }} />}
        {data && <>
          {data.scanErrors.length > 0 && (
            <ConfigMessage message={{ type: 'warning', text: [t('storage.incomplete'), ...data.scanErrors].join('\n') }} />
          )}
          <WorkspaceStorage data={data} />
        </>}
      </ConfigPageSection>
      {data && (
        <ConfigPageSection title={t('storage.cleanup')} bodySurface={false}>
          <div className={styles.panel} data-openbitfun-component="snapshot-storage" data-openbitfun-part="panel">
            <div className={styles.filters} data-openbitfun-component="snapshot-storage" data-openbitfun-part="filters">
              <div className={styles.field}>
                <span>{t('storage.scope')}</span>
                <Select aria-label={t('storage.scope')} size="sm" value={workspace} disabled={busy}
                  onValueChange={v => { setWorkspace(String(v)); setOutcome(null); }}
                  options={[{ value: '', label: t('storage.allWorkspaces') }, ...data.workspaces.map(w => ({
                    value: w.workspaceId, label: `${w.name} · ${w.location}`,
                  }))]} />
              </div>
              <div className={styles.field}>
                <span>{t('storage.period')}</span>
                <Select aria-label={t('storage.period')} size="sm" value={period} disabled={busy}
                  onValueChange={v => {
                    if (period !== v) { setLoading(true); setOutcome(null); setPeriod(v as StoragePeriod); }
                  }}
                  options={[
                    { value: 'thirtyDays', label: t('storage.thirtyDays') },
                    { value: 'sevenDays', label: t('storage.sevenDays') },
                    { value: 'all', label: t('storage.all') },
                  ]} />
              </div>
            </div>
            {loading ? <ConfigLoadingState label={t('storage.scanning')} variant="list" /> : <>
              {rows.filter(row => row.warning || row.unverifiedReferences).map(row => (
                <ConfigMessage key={row.workspaceId} message={{
                  type: 'warning', text: `${row.name} · ${row.warning ?? t('storage.unverified')}`,
                }} />
              ))}
              {count > 0 && <CandidateList rows={rows} />}
              <div className={styles.reviewFooter} data-openbitfun-component="snapshot-storage" data-openbitfun-part="reviewFooter">
                <div>
                  <strong>{count ? preview : t('storage.noCandidates')}</strong>
                  <p>{t('storage.skipped', { sessions: i18nService.formatNumber(skipped) })}</p>
                </div>
                <Button size="sm" variant="outline" tone="danger" disabled={!count || cleaning}
                  leadingIcon={<Trash2 size={14} />} onClick={e => void clean(e.currentTarget)}>
                  {t('storage.cleanup')}
                </Button>
              </div>
            </>}
          </div>
        </ConfigPageSection>
      )}
      <ConfigMessage message={outcome} />
    </>
  );
}

export default function StorageSettingsPage({ isActive = true }: SettingsPageProps) {
  return (
    <SettingsPage pageId="data.storage" data-openbitfun-component="snapshot-storage" data-openbitfun-part="root">
      <FileSnapshotStorageSection isActive={isActive} />
    </SettingsPage>
  );
}
