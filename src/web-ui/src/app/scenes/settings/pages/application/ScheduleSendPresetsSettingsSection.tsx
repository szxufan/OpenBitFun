import { useCallback, useEffect, useMemo, useState } from 'react';
import { Button, IconButton, Input, Select } from '@openbitfun/ui';
import { Plus, RotateCcw, Trash2 } from 'lucide-react';

import { useI18n } from '@/infrastructure/i18n';
import {
  ConfigLoadingState,
  ConfigPageRow,
  ConfigPageSection,
  ConfigRetryState,
} from '@/infrastructure/config/components/common';
import { configManager } from '@/infrastructure/config/services/ConfigManager';
import {
  DEFAULT_SCHEDULE_SEND_PRESETS,
  SCHEDULE_SEND_PRESETS_CONFIG_PATH,
  isValidScheduleSendPreset,
} from '@/infrastructure/config/services/ScheduleSendPresets';
import { useScheduleSendPresets } from '@/infrastructure/config/hooks/useScheduleSendPresets';
import type { ScheduleSendPreset } from '@/infrastructure/config/types';
import { getActiveSurfaceScope } from '@/infrastructure/peer-device/deviceSurface';
import { notificationService } from '@/shared/notification-system';
import { createLogger } from '@/shared/utils/logger';
import { useSettingsSectionAnchor } from '../shared/useSettingsSectionAnchor';

const log = createLogger('ScheduleSendPresetsSettingsSection');

/** Draft rows carry a stable id so React keys survive middle-of-list removals. */
interface DraftRow {
  id: number;
  preset: ScheduleSendPreset;
}

let draftRowIdCounter = 0;
function nextDraftRowId(): number {
  draftRowIdCounter += 1;
  return draftRowIdCounter;
}

function toDraftRows(presets: ScheduleSendPreset[]): DraftRow[] {
  return presets.map(preset => ({ id: nextDraftRowId(), preset }));
}

function presetKind(preset: ScheduleSendPreset): string {
  return preset.kind;
}

function changeKind(preset: ScheduleSendPreset, kind: string): ScheduleSendPreset {
  if (kind === preset.kind) return preset;
  if (kind === 'relative') return { kind: 'relative', offsetMinutes: 15 };
  if (kind === 'daily') return { kind: 'daily', time: '09:00' };
  return { kind: 'tomorrow', time: '09:00' };
}

export default function ScheduleSendPresetsSettingsSection() {
  const { t } = useI18n('settings/application');
  const { presets, error, reload } = useScheduleSendPresets();
  const [draft, setDraft] = useState<DraftRow[]>([]);
  const [dirty, setDirty] = useState(false);
  const [savingEpoch, setSavingEpoch] = useState<number | null>(null);
  const scope = getActiveSurfaceScope();
  const saving = savingEpoch === scope.epoch;
  const sectionAnchor = useSettingsSectionAnchor('schedule-send', presets !== null || !!error);

  useEffect(() => {
    if (presets === null || dirty) return;
    setDraft(toDraftRows(presets));
  }, [presets, dirty]);

  const invalidRowIds = useMemo(
    () => new Set(draft.filter(row => !isValidScheduleSendPreset(row.preset)).map(row => row.id)),
    [draft],
  );

  const updateRow = useCallback((index: number, next: ScheduleSendPreset) => {
    setDraft(current => current.map((row, i) => (i === index ? { ...row, preset: next } : row)));
    setDirty(true);
  }, []);

  const removeRow = useCallback((index: number) => {
    setDraft(current => current.filter((_, i) => i !== index));
    setDirty(true);
  }, []);

  const addRow = useCallback(() => {
    setDraft(current => [...current, { id: nextDraftRowId(), preset: { kind: 'relative', offsetMinutes: 15 } }]);
    setDirty(true);
  }, []);

  const resetToDefaults = useCallback(() => {
    setDraft(toDraftRows(DEFAULT_SCHEDULE_SEND_PRESETS));
    setDirty(true);
  }, []);

  const save = useCallback(async () => {
    if (saving || invalidRowIds.size > 0) return;
    setSavingEpoch(scope.epoch);
    try {
      await configManager.setConfig(SCHEDULE_SEND_PRESETS_CONFIG_PATH, draft.map(row => row.preset));
      setDirty(false);
      notificationService.success(t('scheduleSend.saved'));
    } catch (reason) {
      if (scope.isCurrent()) {
        log.error('Failed to save schedule send presets', reason);
        notificationService.error(t('scheduleSend.saveFailed'));
      }
    } finally {
      if (scope.isCurrent()) setSavingEpoch(null);
    }
  }, [draft, invalidRowIds.size, saving, scope, t]);

  const kindOptions = [
    { value: 'relative', label: t('scheduleSend.typeRelative') },
    { value: 'daily', label: t('scheduleSend.typeDaily') },
    { value: 'tomorrow', label: t('scheduleSend.typeTomorrow') },
  ];

  return (
    <div id={sectionAnchor}>
      <ConfigPageSection
        title={t('scheduleSend.title')}
        description={t('scheduleSend.description')}
        extra={presets !== null ? (
          <div style={{ display: 'flex', gap: 8 }}>
            <Button size="sm" variant="outline" onClick={addRow} leadingIcon={<Plus size={14} />}>
              {t('scheduleSend.add')}
            </Button>
            <Button size="sm" variant="quiet" onClick={resetToDefaults} leadingIcon={<RotateCcw size={14} />}>
              {t('scheduleSend.reset')}
            </Button>
          </div>
        ) : undefined}
      >
        {error ? (
          <ConfigRetryState message={t('scheduleSend.loadFailed')} retryLabel={t('common.retry')} onRetry={() => void reload()} />
        ) : presets === null ? (
          <ConfigLoadingState label={t('scheduleSend.loading')} />
        ) : draft.length === 0 ? (
          <ConfigPageRow label={t('scheduleSend.type')} description={t('scheduleSend.empty')}>
            <span />
          </ConfigPageRow>
        ) : (
          draft.map((row, index) => (
            <ConfigPageRow key={row.id} label={t('scheduleSend.type')}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <Select
                  size="sm"
                  value={presetKind(row.preset)}
                  options={kindOptions}
                  aria-label={t('scheduleSend.type')}
                  onValueChange={value => updateRow(index, changeKind(row.preset, String(value)))}
                />
                {row.preset.kind === 'relative' ? (
                  <Input
                    size="sm"
                    inputMode="numeric"
                    value={String(row.preset.offsetMinutes)}
                    aria-label={t('scheduleSend.minutes')}
                    aria-invalid={invalidRowIds.has(row.id) || undefined}
                    style={{ width: 90 }}
                    onChange={event => {
                      const minutes = Number.parseInt(event.currentTarget.value, 10);
                      updateRow(index, { kind: 'relative', offsetMinutes: Number.isFinite(minutes) ? minutes : 0 });
                    }}
                  />
                ) : (
                  <Input
                    size="sm"
                    value={row.preset.time}
                    aria-label={t('scheduleSend.time')}
                    placeholder="HH:mm"
                    aria-invalid={invalidRowIds.has(row.id) || undefined}
                    style={{ width: 100 }}
                    onChange={event => {
                      const kind = row.preset.kind;
                      if (kind === 'relative') return;
                      updateRow(index, { kind, time: event.currentTarget.value });
                    }}
                  />
                )}
                <IconButton
                  type="button"
                  size="sm"
                  variant="quiet"
                  aria-label={t('scheduleSend.remove')}
                  icon={<Trash2 size={14} />}
                  onClick={() => removeRow(index)}
                />
              </div>
            </ConfigPageRow>
          ))
        )}
        {presets !== null && dirty ? (
          <div style={{ display: 'flex', justifyContent: 'flex-end', alignItems: 'center', gap: 8, paddingTop: 8 }}>
            {invalidRowIds.size > 0 ? (
              <span aria-live="polite">{t('scheduleSend.invalidRow')}</span>
            ) : null}
            <Button
              size="sm"
              variant="fill"
              disabled={saving || invalidRowIds.size > 0}
              onClick={() => void save()}
            >
              {t('scheduleSend.save')}
            </Button>
          </div>
        ) : null}
      </ConfigPageSection>
    </div>
  );
}
