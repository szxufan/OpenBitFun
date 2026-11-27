import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { Combobox } from '@openbitfun/ui';
import { useI18n } from '@/infrastructure/i18n';
import { ConfigPageRow, ConfigRetryState } from '@/infrastructure/config/components/common';
import { useModelSelectPresentation, type ModelSelectOption } from '@/infrastructure/config/components/ModelSelectPresentation';
import { configManager } from '@/infrastructure/config/services/ConfigManager';
import { btwModelSelection, supportsBtwModelDefaults } from '@/infrastructure/config/services/btwModelDefaults';
import { filterSelectableTextChatModels } from '@/infrastructure/config/services/modelCategory';
import type { AIModelConfig, AgentModelTargetSelection } from '@/infrastructure/config/types';
import { modelPoolAPI, readableModelPools, supportsBtwModelPools, type ModelPoolDefinition } from '@/infrastructure/api/service-api/ModelPoolAPI';
import { getActiveSurfaceScope, onSurfaceActivated } from '@/infrastructure/peer-device/deviceSurface';

function BtwModelSettings() {
  const { t } = useI18n('settings/models');
  const { buildModelOption } = useModelSelectPresentation();
  const [scope] = useState(getActiveSurfaceScope);
  const supported = supportsBtwModelDefaults();
  const [value, setValue] = useState<unknown>();
  const [models, setModels] = useState<AIModelConfig[]>([]);
  const [pools, setPools] = useState<ModelPoolDefinition[]>([]);
  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const revision = useRef(0);
  const load = useCallback(async () => {
    const request = ++revision.current;
    try {
      const [config, loadedModels, document] = await Promise.all([
        configManager.getConfig<unknown>('ai.btw'),
        configManager.getOptionalConfig<AIModelConfig[]>('ai.models'),
        supportsBtwModelPools() ? modelPoolAPI.list() : undefined,
      ]);
      if (!scope.isCurrent() || request !== revision.current) return;
      if (!Array.isArray(loadedModels)) throw new Error('Model configuration is unavailable');
      setModels(loadedModels);
      setValue(config);
      setPools(document ? readableModelPools(document) : []);
      setReady(true);
      setError(false);
    } catch {
      if (scope.isCurrent() && request === revision.current) setError(true);
    }
  }, [scope]);
  useEffect(() => {
    if (!supported) return;
    void load();
    const unwatch = configManager.watch('ai.btw', () => void load());
    const unwatchPools = configManager.watch('ai.model_pools', () => void load());
    const unwatchModels = configManager.watch('ai.models', () => void load());
    return () => { revision.current += 1; unwatch(); unwatchPools(); unwatchModels(); };
  }, [load, supported]);

  const selection = ready ? btwModelSelection(value) : undefined;
  const selected = selection?.kind === 'inherit' ? 'inherit'
    : selection?.kind === 'model' ? `model:${selection.model_ref}`
      : selection?.kind === 'pool' ? `pool:${selection.pool_id}` : 'unavailable';
  const options: ModelSelectOption[] = [
    { value: 'inherit', label: t('btwModel.inherit') },
    ...filterSelectableTextChatModels(models).filter(model => model.id?.trim()).map(model => ({ ...buildModelOption(model), value: `model:${model.id}` })),
    ...pools.map(pool => ({ value: `pool:${pool.id}`, label: t('btwModel.pool', { name: pool.name }),
      disabled: pool.member_model_ids.length === 0 || !['ordered', 'balanced'].includes(pool.strategy) })),
  ];
  if (ready && !options.some(option => option.value === selected)) {
    options.push({ value: selected, label: t('btwModel.unavailable'), disabled: true });
  }
  const save = async (next: string) => {
    if (!ready || busy || !scope.isCurrent()) return;
    const choice: AgentModelTargetSelection = next === 'inherit' ? { kind: 'inherit' }
      : next.startsWith('model:') ? { kind: 'model', model_ref: next.slice(6) }
        : { kind: 'pool', pool_id: next.slice(5) };
    setBusy(true);
    revision.current++;
    try {
      const updated = await configManager.updateConfig<unknown>('ai.btw', current => ({
        ...(current && typeof current === 'object' && !Array.isArray(current) ? current : {}),
        model_selection: choice,
      }));
      if (!scope.isCurrent()) return;
      setValue(updated);
      setError(false);
    } catch {
      if (scope.isCurrent()) setError(true);
    } finally {
      if (scope.isCurrent()) setBusy(false);
    }
  };
  return <>
    {!supported ? <p role="status">{t('btwModel.unsupported')}</p> : <>
      <ConfigPageRow label={t('btwModel.label')} align="center">
        <Combobox size="sm" aria-label={t('btwModel.label')} value={ready ? selected : ''}
          options={options} disabled={!ready || busy || error}
          onValueChange={next => void save(String(next))} />
      </ConfigPageRow>
      {error && <ConfigRetryState message={t('btwModel.failed')} retryLabel={t('btwModel.retry')}
        onRetry={() => void load()} />}
    </>}
  </>;
}

export function BtwModelSettingsRow() {
  const scope = useSyncExternalStore(onSurfaceActivated, getActiveSurfaceScope, getActiveSurfaceScope);
  return <BtwModelSettings key={scope.epoch} />;
}
