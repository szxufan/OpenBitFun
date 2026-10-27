import { describe, expect, it } from 'vitest';
import { AccountIdentityControls, useMarketAccountIdentity } from './index';

describe('market account entry (disabled)', () => {
  it('renders no sign-in affordance or identity menu', () => {
    expect(AccountIdentityControls({ showSignIn: true })).toBeNull();
    expect(AccountIdentityControls({})).toBeNull();
    expect(AccountIdentityControls()).toBeNull();
  });

  it('keeps the market identity permanently signed out and resolved', () => {
    expect(useMarketAccountIdentity()).toEqual({
      resolved: true,
      status: 'signed-out',
      me: null,
    });
  });
});
