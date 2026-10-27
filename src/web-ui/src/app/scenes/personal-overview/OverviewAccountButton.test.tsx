/** @vitest-environment jsdom */
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RelayAccountSnapshot } from '@/infrastructure/account-identity';
import { OverviewAccountButton } from './OverviewAccountButton';

const mocks = vi.hoisted(() => ({
  identity: { resolved: true, status: 'signed-out', me: null } as RelayAccountSnapshot,
  logout: vi.fn(),
  error: vi.fn(),
}));
vi.mock('@/infrastructure/account-identity', () => ({
  relayAccountService: { logout: mocks.logout },
}));
vi.mock('@/infrastructure/i18n', () => ({ useI18n: () => ({ t: (key: string) => key }) }));
vi.mock('@/shared/notification-system', () => ({
  useNotification: () => ({ error: mocks.error }),
}));

let root: Root;
let container: HTMLDivElement;
const signedOut: RelayAccountSnapshot = { resolved: true, status: 'signed-out', me: null };
const me = { user: { id: '42', login: '138****0001', avatarUrl: '' } };
const render = (identity = signedOut) => act(async () => {
  mocks.identity = identity;
  root.render(<OverviewAccountButton identity={identity} />);
});

beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.clearAllMocks();
  mocks.logout.mockResolvedValue(undefined);
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe('overview signed-in account', () => {
  it.each(['signed-out', 'loading'] as const)('omits the top-right entry while %s, including cached identity data', async status => {
    await render({ resolved: status !== 'loading', status, me });
    expect(container.childElementCount).toBe(0);
    expect(mocks.logout).not.toHaveBeenCalled();
  });

  it('shows the relay account chip with a letter avatar and signs out directly', async () => {
    await render({ resolved: true, status: 'signed-in', me });
    const button = container.querySelector<HTMLButtonElement>('button')!;
    expect(button.textContent).toContain('accountLogin.logout');
    expect(container.querySelector('.overview-account__name')?.textContent).toContain('138****0001');
    // Relay accounts have no avatar image, so the letter fallback carries the mark.
    expect(container.querySelector('img')).toBeNull();
    expect(container.textContent).toContain('1');
    await act(async () => button.click());
    expect(mocks.logout).toHaveBeenCalledOnce();
  });

  it('reports a failed sign-out instead of failing silently', async () => {
    mocks.logout.mockRejectedValue(new Error('relay unreachable'));
    await render({ resolved: true, status: 'signed-in', me });
    const button = container.querySelector<HTMLButtonElement>('button')!;
    await act(async () => button.click());
    expect(mocks.error).toHaveBeenCalledWith('accountLogin.signOutFailed');
  });
});
