// @vitest-environment jsdom

import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

globalThis.IS_REACT_ACT_ENVIRONMENT = true;

const state = vi.hoisted(() => ({
  peerMode: { active: false } as { active: boolean; deviceId?: string; deviceName?: string },
  getDeviceInfo: vi.fn(),
}));

vi.mock('@/infrastructure/api/service-api/RemoteConnectAPI', () => ({
  remoteConnectAPI: { getDeviceInfo: state.getDeviceInfo },
  deviceDisplayName: (device: { device_id: string; device_name?: string; device_alias?: string }) =>
    device.device_alias ?? device.device_name ?? device.device_id,
}));
vi.mock('@/infrastructure/api/service-api/ApiClient', () => ({ api: { listen: vi.fn() } }));
vi.mock('./peerDeviceContextState', () => ({
  usePeerDeviceModeOptional: () => ({ peerMode: state.peerMode }),
}));

import { useDeviceDirectory } from '@/infrastructure/account/deviceDirectory';
import { activateSurface } from './deviceSurface';
import { resetExecutionDeviceNameForTest, useExecutionDeviceName } from './useExecutionDeviceName';

function Probe() {
  return <span data-testid="name">{useExecutionDeviceName() ?? 'none'}</span>;
}

describe('useExecutionDeviceName', () => {
  let container: HTMLDivElement;
  let root: Root;
  const rendered = () => container.querySelector('[data-testid="name"]')?.textContent;

  beforeEach(() => {
    resetExecutionDeviceNameForTest();
    state.peerMode = { active: false };
    state.getDeviceInfo.mockReset();
    state.getDeviceInfo.mockResolvedValue({ device_id: 'mac-1', device_name: 'MacBook', mac_address: '' });
    useDeviceDirectory.setState({ devices: [], localId: null, status: 'idle' });
    activateSurface('local');
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    activateSurface('local');
  });

  it('names this machine on the local surface, signed out included', async () => {
    await act(async () => root.render(<Probe />));
    expect(rendered()).toBe('MacBook');
    expect(state.getDeviceInfo).toHaveBeenCalledOnce();
  });

  it('prefers the account alias for this machine', async () => {
    useDeviceDirectory.setState({
      devices: [{ device_id: 'mac-1', device_name: 'MacBook', device_alias: 'Work Mac', online: true, last_seen_at: null }],
      localId: 'mac-1',
      status: 'ready',
    });
    await act(async () => root.render(<Probe />));
    expect(rendered()).toBe('Work Mac');
  });

  it('names the controlled device on a peer surface', async () => {
    state.peerMode = { active: true, deviceId: 'ubuntu-1', deviceName: 'Ubuntu' };
    activateSurface('ubuntu-1');
    await act(async () => root.render(<Probe />));
    expect(rendered()).toBe('Ubuntu');
    expect(state.getDeviceInfo).not.toHaveBeenCalled();
  });

  it('keeps the generic label until a name is known', async () => {
    state.getDeviceInfo.mockRejectedValueOnce(new Error('unavailable'));
    await act(async () => root.render(<Probe />));
    expect(rendered()).toBe('none');
  });
});
