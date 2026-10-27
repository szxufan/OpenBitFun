import { describe, expect, it, vi } from 'vitest';
import { ensureAccountSession } from './ensureAccountSession';

describe('shared identity device session', () => {
  it('accepts a relay session that already belongs to the account', async () => {
    const api = { accountStatus: vi.fn().mockResolvedValue({ logged_in: true, user_id: '42' }), accountRefreshRelogin: vi.fn() };
    expect(await ensureAccountSession(api, () => true, '42')).toBe(true);
    expect(api.accountRefreshRelogin).not.toHaveBeenCalled();
  });

  it('renews once from the persisted refresh token when the session is missing', async () => {
    const api = {
      accountStatus: vi.fn()
        .mockResolvedValueOnce({ logged_in: false })
        .mockResolvedValueOnce({ logged_in: true, user_id: '42' }),
      accountRefreshRelogin: vi.fn().mockResolvedValue({ user_id: '42' }),
    };
    expect(await ensureAccountSession(api, () => true, '42')).toBe(true);
    expect(api.accountRefreshRelogin).toHaveBeenCalledOnce();
  });

  it('asks for a fresh SMS sign-in when no refresh token survives', async () => {
    const api = {
      accountStatus: vi.fn().mockResolvedValue({ logged_in: false }),
      accountRefreshRelogin: vi.fn().mockRejectedValue(new Error('session cannot be renewed')),
    };
    expect(await ensureAccountSession(api, () => true, '42')).toBe(false);
  });

  it('treats a session restored for a different account as not ready', async () => {
    const api = {
      accountStatus: vi.fn().mockResolvedValue({ logged_in: true, user_id: '7' }),
      accountRefreshRelogin: vi.fn().mockResolvedValue({ user_id: '7' }),
    };
    expect(await ensureAccountSession(api, () => true, '42')).toBe(false);
  });

  it('does not register after the observing account changes', async () => {
    let current = true;
    const api = {
      accountStatus: vi.fn(async () => { current = false; return { logged_in: false }; }),
      accountRefreshRelogin: vi.fn(),
    };
    expect(await ensureAccountSession(api, () => current, '42')).toBe(false);
    expect(api.accountRefreshRelogin).not.toHaveBeenCalled();
  });

  it('surfaces connection errors without replacing or clearing identity', async () => {
    const api = {
      accountStatus: vi.fn().mockRejectedValue(new Error('offline')),
      accountRefreshRelogin: vi.fn(),
    };
    await expect(ensureAccountSession(api, () => true, '42')).rejects.toThrow('offline');
    expect(api.accountRefreshRelogin).not.toHaveBeenCalled();
  });
});
