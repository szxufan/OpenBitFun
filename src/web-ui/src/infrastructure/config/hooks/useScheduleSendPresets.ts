import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { getActiveSurfaceScope, onSurfaceActivated } from '@/infrastructure/peer-device/deviceSurface';
import { createLogger } from '@/shared/utils/logger';
import { configManager } from '../services/ConfigManager';
import type { ScheduleSendPreset } from '../types';
import {
  getScheduleSendPresets, normalizeScheduleSendPresets,
  SCHEDULE_SEND_PRESETS_CONFIG_PATH,
} from '../services/ScheduleSendPresets';

const log = createLogger('ScheduleSendPresets');

export function useScheduleSendPresets() {
  const scope = useSyncExternalStore(onSurfaceActivated, getActiveSurfaceScope, getActiveSurfaceScope);
  const [snapshot, setSnapshot] = useState<{
    epoch: number; presets: ScheduleSendPreset[] | null; error: Error | null;
  }>({
    epoch: scope.epoch,
    presets: configManager.hasCachedConfig(SCHEDULE_SEND_PRESETS_CONFIG_PATH)
      ? normalizeScheduleSendPresets(configManager.getCachedConfig(SCHEDULE_SEND_PRESETS_CONFIG_PATH)) : null,
    error: null,
  });
  const requestId = useRef(0);
  const reload = useCallback(async () => {
    const id = ++requestId.current;
    try {
      const presets = await getScheduleSendPresets();
      if (scope.isCurrent() && id === requestId.current) {
        setSnapshot({ epoch: scope.epoch, presets, error: null });
      }
    } catch (reason) {
      if (scope.isCurrent() && id === requestId.current) {
        log.warn('Failed to load schedule send presets', reason);
        setSnapshot({ epoch: scope.epoch, presets: null,
          error: reason instanceof Error ? reason : new Error(String(reason)) });
      }
    }
  }, [scope]);

  useEffect(() => {
    void reload();
    const unsubscribe = configManager.watch(SCHEDULE_SEND_PRESETS_CONFIG_PATH, () => void reload());
    return () => { requestId.current += 1; unsubscribe(); };
  }, [reload]);

  return snapshot.epoch === scope.epoch
    ? { presets: snapshot.presets, error: snapshot.error, reload }
    : { presets: null, error: null, reload };
}
