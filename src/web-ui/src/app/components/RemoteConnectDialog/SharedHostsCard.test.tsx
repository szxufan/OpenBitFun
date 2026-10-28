/** @vitest-environment jsdom */
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import SharedHostsCard from './SharedHostsCard';

const mocks = vi.hoisted(() => ({
  accountRelayCapabilities: vi.fn(),
  peerGrantList: vi.fn(),
  peerGrantOffer: vi.fn(),
  peerGrantAccept: vi.fn(),
  peerGrantRevoke: vi.fn(),
  createPeerPskOffer: vi.fn(),
  completePeerPsk: vi.fn(),
  confirmDanger: vi.fn(),
  success: vi.fn(),
  switchToDevice: vi.fn(),
  t: (key: string) => key,
}));

vi.mock('@/infrastructure/api/service-api/RemoteConnectAPI', () => ({ remoteConnectAPI: mocks }));
vi.mock('@/infrastructure/confirm-dialog', () => ({ confirmDanger: mocks.confirmDanger }));
vi.mock('@/shared/notification-system', () => ({ useNotification: () => ({ success: mocks.success }) }));
vi.mock('@/infrastructure/i18n', () => ({
  useI18n: () => ({ t: mocks.t, formatRelativeTime: () => 'recently' }),
}));
vi.mock('@/infrastructure/peer-device/peerDeviceContextState', () => ({
  usePeerDeviceMode: () => ({ switchToDevice: mocks.switchToDevice }),
}));
vi.mock('@openbitfun/ui', () => {
  const Button = ({ children, onClick, disabled, loading }: { children?: React.ReactNode; onClick?: React.MouseEventHandler<HTMLButtonElement>; disabled?: boolean; loading?: boolean }) => (
    <button onClick={onClick} disabled={disabled || loading}>{children}</button>
  );
  const IconButton = ({ 'aria-label': ariaLabel, onClick, disabled }: { 'aria-label'?: string; onClick?: React.MouseEventHandler<HTMLButtonElement>; disabled?: boolean }) => (
    <button aria-label={ariaLabel} onClick={onClick} disabled={disabled} />
  );
  const Input = ({ value, onChange, 'aria-label': ariaLabel }: { value?: string; onChange?: React.ChangeEventHandler<HTMLInputElement>; 'aria-label'?: string }) => (
    <input aria-label={ariaLabel} data-testid={`input:${ariaLabel}`} value={value}
      onChange={onChange} />
  );
  return { Alert: ({ message }: { message: string }) => <div>{message}</div>, Button, Icon: () => null, IconButton, Input };
});

let container: HTMLDivElement;
let root: Root;

const received = [{
  grant: { grant_id: 'g1', owner_user_id: 'owner', device_id: 'host-1', grantee_user_id: 'me', scope: 'drive', state: 'active', offered_at: 1, accepted_at: 1, expires_at: null, revoked_at: null },
  device_name: 'Team Host', device_kind: 'desktop', client_version: null, client_protocol: null, online: true, compatible: true, target_key: 'grant-v1:owner:host-1:g1',
}];
const owned = [{ grant_id: 'g2', owner_user_id: 'me', device_id: 'this-machine', grantee_user_id: 'member', scope: 'observe', state: 'active', offered_at: 1, accepted_at: 1, expires_at: null, revoked_at: null }];

beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.resetAllMocks();
  mocks.confirmDanger.mockResolvedValue(true);
  mocks.accountRelayCapabilities.mockResolvedValue(['device_grants_v1']);
  mocks.peerGrantList.mockResolvedValue({ owned, received });
  mocks.switchToDevice.mockResolvedValue('activated');
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

async function renderCard(onEntered = vi.fn()) {
  await act(async () => { root.render(<SharedHostsCard onEntered={onEntered} />); });
  return onEntered;
}

function findButton(label: string): HTMLButtonElement {
  const button = [...container.querySelectorAll('button')].find(b => (b.textContent ?? '') === label);
  if (!button) throw new Error(`button not found: ${label}`);
  return button as HTMLButtonElement;
}

async function setInput(ariaLabel: string, value: string) {
  const input = container.querySelector<HTMLInputElement>(`[data-testid="input:${ariaLabel}"]`)!;
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!;
  await act(async () => {
    setter.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

describe('SharedHostsCard', () => {
  it('degrades loudly when the relay lacks device_grants_v1', async () => {
    mocks.accountRelayCapabilities.mockResolvedValue(['device_alias_v1']);
    await renderCard();
    expect(container.textContent).toContain('accountLogin.sharedHosts.relayUnsupported');
    expect(mocks.peerGrantList).not.toHaveBeenCalled();
    expect([...container.querySelectorAll('button')].some(b => (b.textContent ?? '') === 'accountLogin.sharedHosts.offerAction')).toBe(false);
  });

  it('enters a shared host through the composite grant target and closes the dialog', async () => {
    const onEntered = await renderCard();
    await act(async () => { findButton('accountLogin.sharedHosts.enterAction').click(); });
    expect(mocks.switchToDevice).toHaveBeenCalledWith('grant-v1:owner:host-1:g1', 'Team Host');
    expect(onEntered).toHaveBeenCalledTimes(1);
  });

  it('disables enter for an offline shared host', async () => {
    mocks.peerGrantList.mockResolvedValue({ owned: [], received: [{ ...received[0], online: false }] });
    await renderCard();
    expect(findButton('accountLogin.sharedHosts.enterAction').disabled).toBe(true);
  });

  it('offers this machine and surfaces the one-time invite secret', async () => {
    mocks.peerGrantOffer.mockResolvedValue({ grantId: 'g3', granteeUserId: 'member', scope: 'drive', inviteSecret: 'SECRET-ONCE', inviteExpiresAt: 2 });
    await renderCard();
    await setInput('accountLogin.sharedHosts.granteePlaceholder', 'member');
    await act(async () => { findButton('accountLogin.sharedHosts.offerAction').click(); });
    expect(mocks.peerGrantOffer).toHaveBeenCalledWith('member', 'drive');
    expect(container.textContent).toContain('SECRET-ONCE');
    expect(mocks.success).toHaveBeenCalledWith('accountLogin.sharedHosts.offerSuccess');
  });

  it('revokes an owned grant only after confirmation', async () => {
    mocks.peerGrantRevoke.mockResolvedValue(undefined);
    await renderCard();
    const revoke = [...container.querySelectorAll('button')].find(b => b.getAttribute('aria-label') === 'accountLogin.sharedHosts.revokeAction')!;
    await act(async () => { revoke.click(); });
    expect(mocks.confirmDanger).toHaveBeenCalled();
    expect(mocks.peerGrantRevoke).toHaveBeenCalledWith('g2');
  });
});
