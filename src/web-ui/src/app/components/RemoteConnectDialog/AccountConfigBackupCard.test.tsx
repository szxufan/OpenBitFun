/** @vitest-environment jsdom */
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import AccountConfigBackupCard from './AccountConfigBackupCard';

const mocks = vi.hoisted(() => ({
  accountConfigBackupStatus: vi.fn(),
  accountConfigBackupExport: vi.fn(),
  accountConfigBackupImport: vi.fn(),
  accountConfigBackupDelete: vi.fn(),
  confirmDanger: vi.fn(),
  success: vi.fn(),
  t: (key: string, params?: Record<string, unknown>) => (
    params ? `${key}:${Object.values(params).join(',')}` : key
  ),
}));

vi.mock('@/infrastructure/api/service-api/RemoteConnectAPI', () => ({
  remoteConnectAPI: mocks,
}));
vi.mock('@/infrastructure/confirm-dialog', () => ({ confirmDanger: mocks.confirmDanger }));
vi.mock('@/shared/notification-system', () => ({ useNotification: () => ({ success: mocks.success }) }));
vi.mock('@/infrastructure/i18n', () => ({
  useI18n: () => ({ t: mocks.t, formatRelativeTime: () => 'recently' }),
}));
vi.mock('@openbitfun/ui', () => {
  const Box = ({ children }: { children?: React.ReactNode }) => <div>{children}</div>;
  const Button = ({ children, onClick, disabled, loading }: { children?: React.ReactNode; onClick?: React.MouseEventHandler<HTMLButtonElement>; disabled?: boolean; loading?: boolean }) => (
    <button onClick={onClick} disabled={disabled || loading} data-loading={loading ? 'true' : undefined}>{children}</button>
  );
  const IconButton = ({ 'aria-label': ariaLabel, onClick }: { 'aria-label'?: string; onClick?: React.MouseEventHandler<HTMLButtonElement> }) => (
    <button aria-label={ariaLabel} onClick={onClick} />
  );
  const Input = ({ value, onChange, onKeyDown, placeholder, 'aria-label': ariaLabel }: { value?: string; onChange?: React.ChangeEventHandler<HTMLInputElement>; onKeyDown?: React.KeyboardEventHandler<HTMLInputElement>; placeholder?: string; 'aria-label'?: string }) => (
    <input aria-label={ariaLabel} data-testid="backup-code-input" onChange={onChange} onKeyDown={onKeyDown} placeholder={placeholder} value={value} />
  );
  return { Alert: ({ message }: { message: string }) => <div>{message}</div>, Button, Icon: () => null, IconButton, Input };
});

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.resetAllMocks();
  mocks.confirmDanger.mockResolvedValue(true);
  mocks.accountConfigBackupStatus.mockResolvedValue({ loggedIn: true, supportedByRelay: true, exists: false });
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

async function renderCard() {
  await act(async () => { root.render(<AccountConfigBackupCard />); });
}

function text(): string {
  return container.textContent ?? '';
}

function findButton(label: string): HTMLButtonElement {
  const button = [...container.querySelectorAll('button')].find(b => (b.textContent ?? '') === label);
  if (!button) throw new Error(`button not found: ${label}`);
  return button as HTMLButtonElement;
}

async function pasteCode(code: string) {
  const input = container.querySelector<HTMLInputElement>('[data-testid="backup-code-input"]')!;
  const nativeSetter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!;
  await act(async () => {
    nativeSetter.call(input, code);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

describe('AccountConfigBackupCard', () => {
  it('shows the empty state when no backup exists and disables delete', async () => {
    await renderCard();
    expect(mocks.accountConfigBackupStatus).toHaveBeenCalledTimes(1);
    expect(text()).toContain('accountLogin.configBackup.none');
    expect(findButton('accountLogin.configBackup.deleteAction').disabled).toBe(true);
  });

  it('degrades loudly when the relay lacks the capability', async () => {
    mocks.accountConfigBackupStatus.mockResolvedValue({ loggedIn: true, supportedByRelay: false, exists: false });
    await renderCard();
    expect(text()).toContain('accountLogin.configBackup.relayUnsupported');
    // No export action on an unsupported relay: the feature must not appear usable.
    expect([...container.querySelectorAll('button')].some(b => (b.textContent ?? '') === 'accountLogin.configBackup.exportAction')).toBe(false);
  });

  it('export shows the one-time code and requires overwrite confirmation when a backup exists', async () => {
    mocks.accountConfigBackupStatus.mockResolvedValue({ loggedIn: true, supportedByRelay: true, exists: true, updatedAt: 1_700_000_000 });
    mocks.accountConfigBackupExport.mockResolvedValue({ code: '056M-TKAD-9N6M', keyHint: 'code_v1:00112233', updatedAt: 1_700_000_100 });
    await renderCard();
    await act(async () => { findButton('accountLogin.configBackup.exportAction').click(); });
    expect(mocks.confirmDanger).toHaveBeenCalledWith(
      'accountLogin.configBackup.overwriteTitle',
      'accountLogin.configBackup.overwriteWarning',
      expect.anything(),
    );
    expect(mocks.accountConfigBackupExport).toHaveBeenCalledTimes(1);
    expect(text()).toContain('accountLogin.configBackup.codeOnceWarning');
    expect(text()).toContain('056M-TKAD-9N6M');
    expect(mocks.success).toHaveBeenCalledWith('accountLogin.configBackup.exportSuccess');
  });

  it('skips export when the overwrite confirmation is dismissed', async () => {
    mocks.accountConfigBackupStatus.mockResolvedValue({ loggedIn: true, supportedByRelay: true, exists: true, updatedAt: 1_700_000_000 });
    mocks.confirmDanger.mockResolvedValue(false);
    await renderCard();
    await act(async () => { findButton('accountLogin.configBackup.exportAction').click(); });
    expect(mocks.accountConfigBackupExport).not.toHaveBeenCalled();
  });

  it('imports with the pasted code and surfaces validation errors as data', async () => {
    mocks.accountConfigBackupImport.mockResolvedValue({ success: false, errors: ['unknown model X', 'bad profile Y'], warnings: [] });
    await renderCard();
    await pasteCode('056M-TKAD');
    await act(async () => { findButton('accountLogin.configBackup.importAction').click(); });
    expect(mocks.confirmDanger).toHaveBeenCalledWith(
      'accountLogin.configBackup.importTitle',
      'accountLogin.configBackup.importWarning',
      expect.anything(),
    );
    expect(mocks.accountConfigBackupImport).toHaveBeenCalledWith('056M-TKAD');
    expect(text()).toContain('unknown model X');
    expect(text()).toContain('bad profile Y');
    expect(mocks.success).not.toHaveBeenCalled();
  });

  it('maps a failed import (wrong code) to a generic loud error without raw detail', async () => {
    mocks.accountConfigBackupImport.mockRejectedValue(new Error('backup code does not match the stored backup'));
    await renderCard();
    await pasteCode('AAAA-AAAA');
    await act(async () => { findButton('accountLogin.configBackup.importAction').click(); });
    expect(text()).toContain('accountLogin.configBackup.importFailed');
    expect(text()).not.toContain('does not match');
  });

  it('deletes the stored backup only after confirmation', async () => {
    mocks.accountConfigBackupStatus.mockResolvedValue({ loggedIn: true, supportedByRelay: true, exists: true, updatedAt: 1_700_000_000 });
    mocks.accountConfigBackupDelete.mockResolvedValue(undefined);
    mocks.confirmDanger.mockResolvedValue(false);
    await renderCard();
    await act(async () => { findButton('accountLogin.configBackup.deleteAction').click(); });
    expect(mocks.accountConfigBackupDelete).not.toHaveBeenCalled();

    mocks.confirmDanger.mockResolvedValue(true);
    await act(async () => { findButton('accountLogin.configBackup.deleteAction').click(); });
    expect(mocks.accountConfigBackupDelete).toHaveBeenCalledTimes(1);
    expect(mocks.success).toHaveBeenCalledWith('accountLogin.configBackup.deleteSuccess');
  });
});
