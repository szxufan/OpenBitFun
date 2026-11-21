// @vitest-environment jsdom
import { expect, it } from 'vitest';
import { surfaceScopedKey } from '@/infrastructure/peer-device/deviceSurface';
import { readWidgetState } from './widgetStateStorage';

it('recovers the live selection when history restores a different text ID', () => {
  localStorage.clear();
  const uri = 'openbitfun://current-session/artifacts/visualizations/v2/layers.html';
  const old = (id: string, call = 'write') =>
    surfaceScopedKey('local', 'visualize.v1', 'session', id, call, uri);
  const stable = surfaceScopedKey('local', 'visualize.v2', 'session', 'turn', 'round', 'write', uri);
  const historic = old('text_1879');
  localStorage.setItem(old('other-round-text-1'), 'unrelated');
  localStorage.setItem(old('round-text-1', 'another-write'), 'unrelated');
  expect(readWidgetState(localStorage, stable, historic, 'round')).toBeNull();
  const selected = JSON.stringify({ privateContent: { selectedLayer: 'runtime' } });
  localStorage.setItem(old('round-text-1'), selected);
  expect(readWidgetState(localStorage, stable, historic, 'round')).toBe(selected);
  expect(localStorage.getItem(stable)).toBe(selected);
  expect(localStorage.getItem(old('round-text-1'))).toBe(selected);
  localStorage.setItem(stable, 'newer');
  expect(readWidgetState(localStorage, stable, historic, 'round')).toBe('newer');
});
