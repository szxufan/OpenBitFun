import { configManager } from './ConfigManager';
import type { ScheduleSendPreset } from '../types';

export const SCHEDULE_SEND_PRESETS_CONFIG_PATH = 'app.flow_chat.schedule_send_presets';

/** Built-in shortcuts used when the user has not customized the list. */
export const DEFAULT_SCHEDULE_SEND_PRESETS: ScheduleSendPreset[] = [
  { kind: 'relative', offsetMinutes: 15 },
  { kind: 'relative', offsetMinutes: 60 },
  { kind: 'daily', time: '20:00' },
  { kind: 'tomorrow', time: '09:00' },
];

const TIME_PATTERN = /^([01]?\d|2[0-3]):[0-5]\d$/;
const MAX_RELATIVE_MINUTES = 24 * 60;

/** Same rules as `normalizePreset`, exposed so the settings editor can flag an
 * invalid row instead of having the next load silently drop it. */
export function isValidScheduleSendPreset(preset: ScheduleSendPreset): boolean {
  if (preset.kind === 'relative') {
    return Number.isFinite(preset.offsetMinutes)
      && preset.offsetMinutes > 0
      && preset.offsetMinutes <= MAX_RELATIVE_MINUTES;
  }
  return TIME_PATTERN.test(preset.time);
}

function normalizePreset(value: unknown): ScheduleSendPreset | null {
  if (!value || typeof value !== 'object') {
    return null;
  }
  const preset = value as Record<string, unknown>;
  if (preset.kind === 'relative') {
    const offset = Number(preset.offsetMinutes);
    if (!Number.isFinite(offset) || offset <= 0 || offset > MAX_RELATIVE_MINUTES) {
      return null;
    }
    return { kind: 'relative', offsetMinutes: Math.round(offset) };
  }
  if (preset.kind === 'daily' || preset.kind === 'tomorrow') {
    const time = typeof preset.time === 'string' ? preset.time.trim() : '';
    if (!TIME_PATTERN.test(time)) {
      return null;
    }
    return { kind: preset.kind, time };
  }
  return null;
}

/**
 * A missing or malformed value falls back to the built-in default so the
 * composer always has usable shortcuts. An explicit empty array is preserved
 * (the popover decides its own fallback) so the settings editor can show the
 * user's real selection.
 */
export function normalizeScheduleSendPresets(value: unknown): ScheduleSendPreset[] {
  if (value === undefined || value === null || !Array.isArray(value)) {
    return DEFAULT_SCHEDULE_SEND_PRESETS.map(preset => ({ ...preset }));
  }
  return value
    .map(normalizePreset)
    .filter((preset): preset is ScheduleSendPreset => preset !== null);
}

/** Optional reads distinguish an older host's missing field from a failed read. */
export async function getScheduleSendPresets(): Promise<ScheduleSendPreset[]> {
  return normalizeScheduleSendPresets(await configManager.getOptionalConfig(SCHEDULE_SEND_PRESETS_CONFIG_PATH));
}

/**
 * Resolve a preset into a concrete local send time relative to `now`. Returns
 * `null` for an unusable preset so the caller can skip it. A `daily` preset
 * that has already passed today rolls over to tomorrow.
 */
export function resolvePresetDate(preset: ScheduleSendPreset, now: Date = new Date()): Date | null {
  const target = new Date(now.getTime());
  if (preset.kind === 'relative') {
    target.setMinutes(target.getMinutes() + preset.offsetMinutes);
    return target;
  }
  const [hours, minutes] = preset.time.split(':').map(Number);
  if (!Number.isFinite(hours) || !Number.isFinite(minutes)) {
    return null;
  }
  if (preset.kind === 'daily') {
    target.setHours(hours, minutes, 0, 0);
    if (target.getTime() <= now.getTime()) {
      target.setDate(target.getDate() + 1);
    }
    return target;
  }
  target.setDate(target.getDate() + 1);
  target.setHours(hours, minutes, 0, 0);
  return target;
}
