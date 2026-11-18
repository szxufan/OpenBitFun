/** @vitest-environment jsdom */
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { beforeEach, afterEach, it, expect, vi } from 'vitest';
import SubagentModelOverrideSection from './SubagentModelOverrideSection';
import { activateSurface } from '@/infrastructure/peer-device/deviceSurface';

const ports = vi.hoisted(() => ({
  read: vi.fn(), save: vi.fn(), peers: vi.fn(),
  changed: undefined as undefined | (() => void),
}));
vi.mock('@/infrastructure/api/service-api/ConfigAPI', () => ({ configAPI: { getConfig: ports.read } }));
vi.mock('@/infrastructure/peer-device/PeerConnectionManager', () => ({ peerConnectionManager: { list: ports.peers } }));
vi.mock('@/infrastructure/config/services/ConfigManager', () => ({ configManager: {
  setConfig: ports.save,
  watch: (_path: string, changed: () => void) => { ports.changed = changed; return () => {}; },
} }));

let container: HTMLDivElement, root: Root;
const mount = () => act(async () => root.render(<SubagentModelOverrideSection />));
const toggle = () => container.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
const click = () => act(async () => toggle().click());
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

beforeEach(() => {
  ports.read.mockReset().mockResolvedValue(true);
  ports.save.mockReset().mockResolvedValue(undefined);
  ports.peers.mockReset().mockReturnValue([{ surfaceId: 'peer-test', capabilities: { subagentModelOverridePolicyV1: true } }]);
  activateSurface('local');
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); activateSurface('local'); });

it('persists an explicit false and can re-enable the host setting', async () => {
  await mount();
  expect(toggle().checked).toBe(true);
  await click();
  expect(ports.save).toHaveBeenLastCalledWith('ai.allow_subagent_model_override', false);
  expect(toggle().checked).toBe(false);
  await click();
  expect(ports.save).toHaveBeenLastCalledWith('ai.allow_subagent_model_override', true);
});

it('locks unsupported peers without reading or writing the controller setting', async () => {
  ports.peers.mockReturnValue([{ surfaceId: 'peer-test', capabilities: {} }]);
  activateSurface('peer-test');
  await mount();
  expect(toggle().disabled).toBe(true);
  expect(ports.read).not.toHaveBeenCalled();
  expect(ports.save).not.toHaveBeenCalled();
});

it('does not enable editing when an older server omits the field', async () => {
  ports.read.mockResolvedValue(undefined);
  await mount();
  expect(toggle().disabled).toBe(true);
  expect(ports.save).not.toHaveBeenCalled();
});

it('preserves the committed setting on write failure', async () => {
  ports.save.mockRejectedValueOnce(new Error('Offline'));
  await mount();
  await click();
  expect(toggle().checked).toBe(true);
  expect(container.querySelector('button')).not.toBeNull();
});

it('keeps failed reads uneditable and supports retry', async () => {
  ports.read.mockRejectedValueOnce(new Error('Offline'));
  await mount();
  expect(toggle()).toBeNull();
  ports.read.mockResolvedValue(false);
  await act(async () => container.querySelector<HTMLButtonElement>('button')!.click());
  expect(toggle().checked).toBe(false);
});

it('discards a previous host read after switching to a peer', async () => {
  const stale = deferred<boolean>();
  ports.read.mockReturnValueOnce(stale.promise);
  await mount();
  ports.read.mockResolvedValue(false);
  await act(async () => activateSurface('peer-test'));
  await act(async () => stale.resolve(true));
  expect(toggle().checked).toBe(false);
});

it('discards a previous host save after switching to a peer', async () => {
  await mount();
  const pending = deferred<void>();
  ports.save.mockReturnValueOnce(pending.promise);
  await click();
  ports.read.mockResolvedValue(true);
  await act(async () => activateSurface('peer-test'));
  await act(async () => pending.resolve());
  expect(toggle().checked).toBe(true);
});

it('refreshes after host configuration changes without overriding a newer save', async () => {
  await mount();
  const stale = deferred<boolean>();
  ports.read.mockReturnValueOnce(stale.promise);
  await act(async () => ports.changed!());
  await click();
  await act(async () => stale.resolve(true));
  expect(toggle().checked).toBe(false);
});
