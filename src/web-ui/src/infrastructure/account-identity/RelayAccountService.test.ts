// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  RelayAccountService,
  type RelayAccountChangedEvent,
  type RelayAccountServiceDependencies,
  type RelayAccountSyncPort,
} from './RelayAccountService';

class FakeSyncPort implements RelayAccountSyncPort {
  readonly published: RelayAccountChangedEvent[] = [];
  private readonly listeners = new Set<(event: RelayAccountChangedEvent) => void>();

  publish(event: RelayAccountChangedEvent): void {
    this.published.push(event);
  }

  subscribe(listener: (event: RelayAccountChangedEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  emit(event: RelayAccountChangedEvent): void {
    this.listeners.forEach(listener => listener(event));
  }
}

function setup() {
  const syncPort = new FakeSyncPort();
  const api = {
    status: vi.fn<() => Promise<{ logged_in: boolean; user_id?: string | null }>>()
      .mockResolvedValue({ logged_in: false }),
    credentialHint: vi.fn().mockResolvedValue(null),
    smsSend: vi.fn().mockResolvedValue(undefined),
    loginWithCode: vi.fn().mockResolvedValue({ user_id: 'user-1' }),
    loginWithPassword: vi.fn().mockResolvedValue({ user_id: 'user-1' }),
    refreshRelogin: vi.fn().mockResolvedValue({ user_id: 'user-1' }),
    setPassword: vi.fn().mockResolvedValue(undefined),
    logout: vi.fn().mockResolvedValue(undefined),
    onLoginState: vi.fn(() => () => undefined),
  };
  const dependencies: RelayAccountServiceDependencies = {
    api,
    syncPort,
    sourceId: 'window-a',
  };
  const service = new RelayAccountService(dependencies);
  return { api, dependencies, service, syncPort };
}

const activeServices: RelayAccountService[] = [];

afterEach(() => {
  activeServices.splice(0).forEach(service => service.dispose());
});

describe('RelayAccountService', () => {
  it('resolves to signed-out when the host has no session', async () => {
    const { service } = setup();
    activeServices.push(service);
    await service.initialize();
    expect(service.getSnapshot()).toMatchObject({ resolved: true, status: 'signed-out', me: null });
  });

  it('survives an initial status failure as signed-out with the error kept', async () => {
    const { api, service } = setup();
    activeServices.push(service);
    api.status.mockRejectedValueOnce(new Error('transport down'));
    await service.initialize();
    expect(service.getSnapshot()).toMatchObject({ resolved: true, status: 'signed-out', me: null });
    expect(service.getSnapshot().lastError?.message).toBe('transport down');
  });

  it('signs in with an SMS code, shares the change, and shows the masked phone', async () => {
    const { api, service, syncPort } = setup();
    activeServices.push(service);
    await service.initialize();
    api.credentialHint.mockResolvedValue({ username: '138****0001', relay_url: 'https://relay.example' });

    await expect(service.signInWithCode('13800138000', '123456')).resolves.toEqual({ user_id: 'user-1' });

    expect(api.loginWithCode).toHaveBeenCalledWith('13800138000', '123456');
    await vi.waitFor(() => expect(service.getSnapshot().me).toEqual({
      user: { id: 'user-1', login: '138****0001', avatarUrl: '' },
    }));
    expect(service.getSnapshot()).toMatchObject({ resolved: true, status: 'signed-in' });
    expect(syncPort.published).toEqual([
      expect.objectContaining({ kind: 'identity-changed', sourceId: 'window-a' }),
    ]);
  });

  it('keeps the relay user id as the display name until the hint answers', async () => {
    const { api, service } = setup();
    activeServices.push(service);
    api.credentialHint.mockImplementation(() => new Promise(() => {}));
    await service.initialize();
    void service.signInWithPassword('13800138000', 'hunter2');
    await vi.waitFor(() => expect(service.getSnapshot().me).toEqual({
      user: { id: 'user-1', login: 'user-1', avatarUrl: '' },
    }));
  });

  it('refreshes from another window and broadcasts logout from this one', async () => {
    const { api, service, syncPort } = setup();
    activeServices.push(service);
    await service.initialize();
    api.status.mockResolvedValue({ logged_in: true, user_id: 'user-7' });

    syncPort.emit({ kind: 'identity-changed', eventId: 'remote-1', sourceId: 'window-b' });
    await vi.waitFor(() => expect(service.getSnapshot().me?.user.id).toBe('user-7'));

    await service.logout();
    expect(api.logout).toHaveBeenCalledOnce();
    expect(service.getSnapshot()).toMatchObject({ status: 'signed-out', me: null });
    expect(syncPort.published.at(-1)).toEqual(expect.objectContaining({
      kind: 'identity-changed',
      sourceId: 'window-a',
    }));
  });

  it('ignores a status answer that started before logout', async () => {
    const { api, service } = setup();
    activeServices.push(service);
    await service.initialize();
    let resolve!: (value: { logged_in: boolean; user_id?: string }) => void;
    api.status.mockImplementationOnce(() => new Promise(done => { resolve = done; }));
    const stale = service.refresh(false);
    await service.logout();
    resolve({ logged_in: true, user_id: 'user-1' });
    await stale;
    expect(service.getSnapshot()).toMatchObject({ status: 'signed-out', me: null });
  });

  it('keeps a late initial status failure from erasing a newer sign-in', async () => {
    const { api, service } = setup();
    activeServices.push(service);
    let reject!: (reason: Error) => void;
    api.status.mockImplementationOnce(() => new Promise((_, fail) => { reject = fail; }));
    const stale = service.initialize();
    await service.signInWithCode('13800138000', '123456');
    reject(new Error('old network failure'));
    await stale;
    expect(service.getSnapshot()).toMatchObject({ status: 'signed-in', me: { user: { id: 'user-1' } } });
  });

  it('renews an expired session through the persisted refresh token', async () => {
    const { api, service } = setup();
    activeServices.push(service);
    await service.initialize();
    await expect(service.relogin()).resolves.toEqual({ user_id: 'user-1' });
    expect(api.refreshRelogin).toHaveBeenCalledOnce();
    expect(service.getSnapshot()).toMatchObject({ status: 'signed-in', me: { user: { id: 'user-1' } } });
  });

  it('rejects relogin when no refresh token survives, for the caller to demand a new code', async () => {
    const { api, service } = setup();
    activeServices.push(service);
    await service.initialize();
    api.refreshRelogin.mockRejectedValue(new Error('sign in again with a verification code'));
    await expect(service.relogin()).rejects.toThrow('sign in again with a verification code');
    expect(service.getSnapshot()).toMatchObject({ status: 'signed-out', me: null });
  });
});
