/**
 * Reconcile the relay account identity with the host's relay session.
 *
 * The identity and the relay session come from the same host state, so a
 * mismatch means the session was cleared under us (expiry, host restart).
 * Renew once from the persisted refresh token; if that cannot restore this
 * account, only a fresh SMS-code sign-in can.
 */
export async function ensureAccountSession(
  api: {
    accountStatus(): Promise<{ logged_in: boolean; user_id?: string | null }>;
    accountRefreshRelogin(): Promise<unknown>;
  },
  isCurrent: () => boolean,
  accountId: string,
): Promise<boolean> {
  if (!isCurrent()) return false;
  let status = await api.accountStatus();
  if (!isCurrent()) return false;
  if (!matchesSession(status, accountId)) {
    try {
      await api.accountRefreshRelogin();
      status = await api.accountStatus();
      if (!isCurrent()) return false;
    } catch {
      // No refresh token survived; the caller must offer a new SMS sign-in.
      return false;
    }
  }
  return matchesSession(status, accountId);
}

function matchesSession(
  status: { logged_in: boolean; user_id?: string | null },
  accountId: string,
): boolean {
  return Boolean(status.logged_in) && status.user_id === String(accountId);
}
