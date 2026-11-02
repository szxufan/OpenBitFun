import { describe, expect, it } from 'vitest';
import { formatContextLength } from './modelConfigs';

describe('formatContextLength', () => {
  it('uses 1024-based units when they land on round numbers', () => {
    expect(formatContextLength(1048576)).toBe('1M');
    expect(formatContextLength(2097152)).toBe('2M');
    expect(formatContextLength(131072)).toBe('128K');
    expect(formatContextLength(32768)).toBe('32K');
    expect(formatContextLength(65536)).toBe('64K');
    expect(formatContextLength(8192)).toBe('8K');
  });

  it('falls back to 1000-based K when 1024 is not round', () => {
    expect(formatContextLength(200000)).toBe('200K');
    expect(formatContextLength(128000)).toBe('125K');
    expect(formatContextLength(48000)).toBe('48K');
  });

  it('rounds near-1M budgets to M', () => {
    expect(formatContextLength(1006592)).toBe('1M');
    expect(formatContextLength(1000000)).toBe('1M');
  });

  it('returns the raw count below 1K tokens and never emits 0K/0M', () => {
    expect(formatContextLength(512)).toBe('512');
    expect(formatContextLength(32768)).not.toBe('0M');
    expect(formatContextLength(0)).toBe('');
  });
});
