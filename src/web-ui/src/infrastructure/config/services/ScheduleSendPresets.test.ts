import { describe, expect, it } from 'vitest';
import {
  DEFAULT_SCHEDULE_SEND_PRESETS,
  isValidScheduleSendPreset,
  normalizeScheduleSendPresets,
  resolvePresetDate,
} from './ScheduleSendPresets';

describe('isValidScheduleSendPreset', () => {
  it('accepts presets the normalizer would keep', () => {
    expect(isValidScheduleSendPreset({ kind: 'relative', offsetMinutes: 30 })).toBe(true);
    expect(isValidScheduleSendPreset({ kind: 'daily', time: '09:05' })).toBe(true);
    expect(isValidScheduleSendPreset({ kind: 'tomorrow', time: '23:59' })).toBe(true);
    DEFAULT_SCHEDULE_SEND_PRESETS.forEach(preset => {
      expect(isValidScheduleSendPreset(preset)).toBe(true);
    });
  });

  it('rejects the same values the normalizer drops', () => {
    expect(isValidScheduleSendPreset({ kind: 'relative', offsetMinutes: 0 })).toBe(false);
    expect(isValidScheduleSendPreset({ kind: 'relative', offsetMinutes: 24 * 60 + 1 })).toBe(false);
    expect(isValidScheduleSendPreset({ kind: 'relative', offsetMinutes: Number.NaN })).toBe(false);
    expect(isValidScheduleSendPreset({ kind: 'daily', time: '25:00' })).toBe(false);
    expect(isValidScheduleSendPreset({ kind: 'daily', time: '9:0' })).toBe(false);
    expect(isValidScheduleSendPreset({ kind: 'tomorrow', time: '' })).toBe(false);
  });
});

describe('normalizeScheduleSendPresets', () => {
  it('falls back to the built-in defaults for a missing or malformed value', () => {
    expect(normalizeScheduleSendPresets(undefined)).toEqual(DEFAULT_SCHEDULE_SEND_PRESETS);
    expect(normalizeScheduleSendPresets(null)).toEqual(DEFAULT_SCHEDULE_SEND_PRESETS);
    expect(normalizeScheduleSendPresets('not-a-list')).toEqual(DEFAULT_SCHEDULE_SEND_PRESETS);
  });

  it('drops malformed presets and keeps valid ones in order', () => {
    expect(normalizeScheduleSendPresets([
      { kind: 'relative', offsetMinutes: 0 },
      { kind: 'relative', offsetMinutes: 30 },
      { kind: 'daily', time: '25:00' },
      { kind: 'tomorrow', time: '09:30' },
      { kind: 'unknown' },
    ])).toEqual([
      { kind: 'relative', offsetMinutes: 30 },
      { kind: 'tomorrow', time: '09:30' },
    ]);
  });

  it('preserves an explicit empty list', () => {
    expect(normalizeScheduleSendPresets([])).toEqual([]);
  });
});

describe('resolvePresetDate', () => {
  // Local-time anchor: Jan 15 2026, 10:00.
  const now = new Date(2026, 0, 15, 10, 0, 0);

  it('adds the relative offset in minutes', () => {
    const date = resolvePresetDate({ kind: 'relative', offsetMinutes: 45 }, now);
    expect(date?.getDate()).toBe(15);
    expect(date?.getHours()).toBe(10);
    expect(date?.getMinutes()).toBe(45);
  });

  it('rolls an already-passed daily time to tomorrow', () => {
    const date = resolvePresetDate({ kind: 'daily', time: '09:00' }, now);
    expect(date?.getDate()).toBe(16);
    expect(date?.getHours()).toBe(9);
    expect(date?.getMinutes()).toBe(0);
  });

  it('keeps a still-future daily time today', () => {
    const date = resolvePresetDate({ kind: 'daily', time: '20:00' }, now);
    expect(date?.getDate()).toBe(15);
    expect(date?.getHours()).toBe(20);
  });

  it('resolves tomorrow at the given time', () => {
    const date = resolvePresetDate({ kind: 'tomorrow', time: '09:00' }, now);
    expect(date?.getDate()).toBe(16);
    expect(date?.getHours()).toBe(9);
  });
});
