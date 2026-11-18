import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { Switch } from '@openbitfun/ui';
import { ConfigLoadingState, ConfigPageRow, ConfigRetryState } from '@/infrastructure/config/components/common';
import { configManager } from '@/infrastructure/config/services/ConfigManager';
import { readSubagentModelPolicy, subagentModelOverridePath, writeSubagentModelPolicy } from '@/infrastructure/config/services/subagentModelPolicy';
import { getActiveSurfaceScope, onSurfaceActivated } from '@/infrastructure/peer-device/deviceSurface';
import { useI18n } from '@/infrastructure/i18n';
import { createLogger } from '@/shared/utils/logger';

const log = createLogger('SubagentModelOverrideSettings');

export default function SubagentModelOverrideSection() {
  const { t } = useI18n('settings/agentic-tools');
  const scope = useSyncExternalStore(onSurfaceActivated, getActiveSurfaceScope, getActiveSurfaceScope);
  const [loaded, setLoaded] = useState<{ epoch: number; allowed: boolean | undefined } | null>(null);
  const [error, setError] = useState<'load' | 'save' | null>(null);
  const [saving, setSaving] = useState(false);
  const request = useRef(0);
  const savingRef = useRef(false);
  const invalidateRequests = useCallback(() => {
    request.current++;
  }, []);
  const load = useCallback(async () => {
    const current = ++request.current;
    setError(null);
    try {
      const allowed = await readSubagentModelPolicy();
      if (scope.isCurrent() && current === request.current) setLoaded({ epoch: scope.epoch, allowed });
    } catch (failure) {
      if (scope.isCurrent() && current === request.current) {
        log.error('Failed to load subagent model override policy', failure);
        setError('load');
      }
    }
  }, [scope]);

  useEffect(() => {
    setLoaded(null);
    setSaving(false);
    savingRef.current = false;
    void load();
    const stop = configManager.watch(subagentModelOverridePath, () => {
      if (!savingRef.current) void load();
    });
    return () => {
      invalidateRequests();
      stop();
    };
  }, [invalidateRequests, load]);

  const ready = loaded?.epoch === scope.epoch;
  const save = async (allowed: boolean) => {
    if (!scope.isCurrent() || !ready || loaded.allowed === undefined || savingRef.current) return;
    invalidateRequests();
    savingRef.current = true;
    setSaving(true);
    setError(null);
    try {
      await writeSubagentModelPolicy(allowed);
      if (scope.isCurrent()) setLoaded({ epoch: scope.epoch, allowed });
    } catch (failure) {
      if (scope.isCurrent()) {
        log.error('Failed to save subagent model override policy', failure);
        setError('save');
      }
    } finally {
      if (scope.isCurrent()) { savingRef.current = false; setSaving(false); }
    }
  };

  return <>
    <ConfigPageRow label={t('config.subagentModelOverride.label')}
      description={ready && loaded.allowed === undefined
        ? t('config.subagentModelOverride.unsupported') : t('config.subagentModelOverride.hint')}
      align="center">
      {ready
        ? <Switch aria-label={t('config.subagentModelOverride.label')} checked={loaded.allowed === true}
          disabled={saving || loaded.allowed === undefined || error === 'load'}
          onChange={event => void save(event.target.checked)} />
        : !error && <ConfigLoadingState label={t('config.subagentModelOverride.loading')} />}
    </ConfigPageRow>
    {error && <ConfigRetryState
      message={error === 'load' ? t('config.subagentModelOverride.loadFailed') : t('config.subagentModelOverride.saveFailed')}
      retryLabel={t('config.subagentModelOverride.retry')} onRetry={() => void load()} />}
  </>;
}
