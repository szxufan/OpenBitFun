import type { RelayAccountSnapshot } from '@/infrastructure/account-identity/RelayAccountService';

/**
 * Display identity for the relay account. The masked phone from the credential
 * hint is the display name; there is no separate handle to show.
 */
export function getOverviewIdentity(identity: RelayAccountSnapshot) {
  if (!identity.resolved || identity.status !== 'signed-in' || !identity.me) return null;
  const name = identity.me.user.login.trim();
  return name ? { name, handle: null } : null;
}
