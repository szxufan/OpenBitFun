import { describe, expect, it } from 'vitest';
import type { RelayAccountSnapshot } from '@/infrastructure/account-identity/RelayAccountService';
import { getOverviewIdentity } from './overviewIdentity';

const identity: RelayAccountSnapshot = {
  resolved: true,
  status: 'signed-in',
  me: { user: { id: 'user-1', login: '138****0001', avatarUrl: '' } },
};

describe('personal overview identity', () => {
  it('shows the masked phone of the signed-in relay account', () => {
    expect(getOverviewIdentity(identity)).toEqual({ name: '138****0001', handle: null });
  });

  it.each(['signed-out', 'loading'] as const)('hides the identity while %s', status => {
    expect(getOverviewIdentity({ ...identity, status, resolved: status !== 'loading' })).toBeNull();
  });

  it('hides the identity when the relay reports no session', () => {
    expect(getOverviewIdentity({ resolved: true, status: 'signed-out', me: null })).toBeNull();
  });

  it('falls back to the relay user id while the credential hint has not answered', () => {
    expect(getOverviewIdentity({
      ...identity,
      me: { user: { id: 'u-9', login: 'u-9', avatarUrl: '' } },
    })).toEqual({ name: 'u-9', handle: null });
  });
});
