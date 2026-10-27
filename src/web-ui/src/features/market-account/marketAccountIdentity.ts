import type { RelayAccountSnapshot } from '@/infrastructure/account-identity/RelayAccountService';

const MARKET_ACCOUNT_IDENTITY: RelayAccountSnapshot = Object.freeze({
  resolved: true,
  status: 'signed-out',
  me: null,
});

/**
 * Market surfaces no longer expose an account entry: browsing stays anonymous
 * and sign-in affordances are removed. This hook pins that decision in code —
 * the market identity is permanently signed out, whatever the relay account
 * state is, so personalized market actions stay gated off instead of quietly
 * re-attaching to the relay-local account.
 */
export function useMarketAccountIdentity(): RelayAccountSnapshot {
  return MARKET_ACCOUNT_IDENTITY;
}
