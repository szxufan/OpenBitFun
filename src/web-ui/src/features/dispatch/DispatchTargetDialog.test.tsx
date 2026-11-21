// @vitest-environment jsdom

import {
  BASE_DISPATCH_CAPABILITIES,
  DISPATCH_PROTOCOL_VERSION,
} from './dispatchPreflight';

import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DispatchTargetDialog } from './DispatchTargetDialog';

globalThis.IS_REACT_ACT_ENVIRONMENT = true;

/**
 * What a usable target advertises. The approval policy is switched in the
 * composer between turns, so setup accepts a target only when it can serve
 * every policy the user may later pick.
 */
const READY_CAPABILITIES = [
  ...BASE_DISPATCH_CAPABILITIES,
  'approval_auto',
  'approval_reject_and_report',
  'approval_remote',
];

const mocks = vi.hoisted(() => ({
  probeTarget: vi.fn(),
  getFreshConfig: vi.fn(),
  resolveRevision: vi.fn(),
  dialogOnOpenChange: null as ((open: boolean) => void) | null,
  dialogLifecycleProps: null as {
    closeOnPointerOutside?: boolean;
    showCloseButton?: boolean;
  } | null,
}));

vi.mock('./dispatchApi', () => ({
  dispatchApi: {
    probeTarget: mocks.probeTarget,
  },
}));

vi.mock('@/infrastructure/i18n', () => ({
  useI18n: () => ({
    t: (key: string) => key,
  }),
}));

vi.mock('@/infrastructure/api/service-api/ConfigAPI', () => ({
  configAPI: { getConfig: mocks.getFreshConfig },
}));

vi.mock('@/infrastructure/api/service-api/GitAPI', () => ({
  gitAPI: { resolveRevision: mocks.resolveRevision },
}));

vi.mock('@openbitfun/ui', async (importOriginal) => ({
  Disclosure: (await importOriginal<typeof import('@openbitfun/ui')>()).Disclosure,
  Alert: ({ message }: { message: string }) => <div role="alert">{message}</div>,
  Icon: ({ name, ...props }: { name: string } & React.HTMLAttributes<HTMLSpanElement>) => <span data-icon={name} {...props} />,
  ScrollArea: ({ children, ...props }: React.HTMLAttributes<HTMLDivElement>) => <div {...props}>{children}</div>,
  Input: ({ onChange, onValueChange, ...props }: React.InputHTMLAttributes<HTMLInputElement> & {
    onValueChange?: (value: string) => void;
  }) => (
    <input
      {...props}
      onChange={(event) => {
        onChange?.(event);
        onValueChange?.(event.currentTarget.value);
      }}
    />
  ),
  Checkbox: ({ description, label, onChange, onCheckedChange, ...props }: React.InputHTMLAttributes<HTMLInputElement> & {
    description?: React.ReactNode;
    label?: React.ReactNode;
    onCheckedChange?: (checked: boolean) => void;
  }) => (
    <label>
      <input
        {...props}
        type="checkbox"
        onChange={(event) => {
          onChange?.(event);
          onCheckedChange?.(event.currentTarget.checked);
        }}
      />
      <span>{label}</span>
      <small>{description}</small>
    </label>
  ),
  Button: ({
    children,
    disabled,
    onClick,
  }: React.PropsWithChildren<{
    disabled?: boolean;
    onClick?: React.MouseEventHandler<HTMLButtonElement>;
  }>) => (
    <button type="button" disabled={disabled} onClick={onClick}>
      {children}
    </button>
  ),
  Dialog: ({
    children,
    closeOnPointerOutside,
    onOpenChange,
    open,
  }: React.PropsWithChildren<{
    closeOnPointerOutside?: boolean;
    onOpenChange: (open: boolean) => void;
    open: boolean;
  }>) => {
    mocks.dialogOnOpenChange = onOpenChange;
    mocks.dialogLifecycleProps = {
      closeOnPointerOutside,
      showCloseButton: false,
    };
    return open ? <div role="dialog">{children}</div> : null;
  },
  DialogBody: ({ children }: React.PropsWithChildren) => <div>{children}</div>,
  DialogClose: (props: React.ButtonHTMLAttributes<HTMLButtonElement>) => {
    if (mocks.dialogLifecycleProps) mocks.dialogLifecycleProps.showCloseButton = true;
    return <button type="button" {...props} />;
  },
  DialogHeader: ({ children }: React.PropsWithChildren) => <header>{children}</header>,
  DialogHeading: ({ children }: React.PropsWithChildren) => <div>{children}</div>,
  DialogTitle: ({ children }: React.PropsWithChildren) => <h2>{children}</h2>,
  DialogDescription: ({ children }: React.PropsWithChildren) => <p>{children}</p>,
  DialogFooter: ({ children }: React.PropsWithChildren) => <footer>{children}</footer>,
}));

function createDeferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, reject, resolve };
}

describe('DispatchTargetDialog target preparation', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.clearAllMocks();
    mocks.dialogOnOpenChange = null;
    mocks.dialogLifecycleProps = null;
    mocks.probeTarget.mockResolvedValue({
      cliInstalled: false,
      os: 'linux',
      arch: 'x86_64',
      installSupported: true,
      release: {
        version: '1.2.3',
        target: 'x86_64-unknown-linux-gnu',
        url: 'https://example.test/openbitfun',
        sha256: 'abc123',
      },
    });
    mocks.getFreshConfig.mockResolvedValue(undefined);
    mocks.resolveRevision.mockResolvedValue('a'.repeat(40));
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it('does not overwrite a user choice when the worktree default resolves late', async () => {
    const worktreeSettings = createDeferred<{ copyLocalChanges: boolean }>();
    mocks.getFreshConfig.mockReturnValue(worktreeSettings.promise);

    await act(async () => {
      root.render(
        <DispatchTargetDialog
          open
          target={{ kind: 'ssh', connectionId: 'ssh-1', displayName: 'build-host' }}
          sourceWorkspacePath="/home/me/project"
          sourceWorkspaceId="workspace-source"
          onClose={vi.fn()}
          onReady={vi.fn()}
        />,
      );
      await Promise.resolve();
    });

    const includeUncommitted = container.querySelector<HTMLInputElement>(
      'input[type="checkbox"]',
    );
    expect(includeUncommitted?.checked).toBe(false);

    await act(async () => {
      includeUncommitted?.click();
    });
    expect(includeUncommitted?.checked).toBe(true);

    await act(async () => {
      worktreeSettings.resolve({ copyLocalChanges: false });
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(includeUncommitted?.checked).toBe(true);
  });

  it('keeps setup open and reports an invalid base revision before creating a session', async () => {
    const onReady = vi.fn();
    mocks.resolveRevision.mockRejectedValue(new Error('unknown revision'));
    mocks.probeTarget.mockResolvedValue({
      cliInstalled: true,
      os: 'linux',
      arch: 'x86_64',
      installSupported: false,
      protocol: {
        productId: 'openbitfun',
        dataNamespace: 'openbitfun',
        protocolVersion: DISPATCH_PROTOCOL_VERSION,
        cliVersion: '1.2.3',
        os: 'linux',
        arch: 'x86_64',
        capabilities: READY_CAPABILITIES,
        modelConfigured: true,
        availableModels: ['model-a'],
      },
    });

    await act(async () => {
      root.render(
        <DispatchTargetDialog
          open
          target={{ kind: 'ssh', connectionId: 'ssh-1', displayName: 'build-host' }}
          sourceWorkspacePath="/home/me/project"
          sourceWorkspaceId="workspace-source"
          onClose={vi.fn()}
          onReady={onReady}
        />,
      );
      await Promise.resolve();
      await Promise.resolve();
    });

    const baseRefInput = container.querySelector<HTMLInputElement>(
      '.dispatch-install-dialog__base-ref input',
    );
    await act(async () => {
      if (baseRefInput) {
        Object.getOwnPropertyDescriptor(
          HTMLInputElement.prototype,
          'value',
        )?.set?.call(baseRefInput, 'missing/ref');
        baseRefInput.dispatchEvent(new Event('input', { bubbles: true }));
      }
    });

    await act(async () => {
      Array.from(container.querySelectorAll('button'))
        .find(button => button.textContent?.includes('dispatch.useTarget'))
        ?.click();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mocks.resolveRevision).toHaveBeenCalledWith(
      { workspaceId: 'workspace-source', repositoryPath: '/home/me/project' },
      'missing/ref',
    );
    expect(onReady).not.toHaveBeenCalled();
    expect(container.textContent).toContain('dispatch.baseRefInvalid');
    expect(container.querySelector('.dispatch-install-dialog')).not.toBeNull();
  });

  it('blocks a missing CLI even when an older host advertises one-click setup', async () => {
    mocks.probeTarget.mockResolvedValue({
      cliInstalled: false,
      os: 'linux',
      arch: 'x86_64',
      installSupported: true,
      prebuiltIncompatible: 'target uses musl libc',
    });

    await act(async () => {
      root.render(
        <DispatchTargetDialog
          open
          target={{ kind: 'ssh', connectionId: 'ssh-1', displayName: 'alpine-host' }}
          sourceWorkspacePath="/home/me/project"
          sourceWorkspaceId="workspace-source"
          onClose={vi.fn()}
          onReady={vi.fn()}
        />,
      );
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(container.textContent).toContain('dispatch.cliUnavailable');
    expect(container.textContent).toContain('dispatch.retryCheck');
    expect(container.textContent).not.toContain('target uses musl libc');
    expect(container.textContent).not.toContain('sourceBuild');
    expect(container.textContent).not.toContain('dispatch.oneClickDeploy');

    const useTarget = Array.from(container.querySelectorAll('button'))
      .find(button => button.textContent?.includes('dispatch.useTarget'));
    expect(
      useTarget?.disabled,
      'a target that cannot be prepared must not be selectable',
    ).toBe(true);
  });

  it('keeps protocol capability names and probe failures out of the user interface', async () => {
    mocks.probeTarget.mockResolvedValueOnce({
      cliInstalled: true,
      os: 'linux',
      arch: 'x86_64',
      installSupported: false,
      protocol: {
        productId: 'openbitfun',
        dataNamespace: 'openbitfun',
        protocolVersion: DISPATCH_PROTOCOL_VERSION,
        cliVersion: '1.2.3',
        os: 'linux',
        arch: 'x86_64',
        capabilities: READY_CAPABILITIES.filter(
          capability => capability !== 'workspace_git_sync',
        ),
        modelConfigured: true,
        availableModels: ['model-a'],
      },
    });

    await act(async () => {
      root.render(
        <DispatchTargetDialog
          open
          target={{ kind: 'ssh', connectionId: 'ssh-1', displayName: 'build-host' }}
          sourceWorkspacePath="/home/me/project"
          sourceWorkspaceId="workspace-source"
          onClose={vi.fn()}
          onReady={vi.fn()}
        />,
      );
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(container.textContent).toContain('dispatch.cliUpdateRequired');
    expect(container.textContent).not.toContain('workspace_git_sync');

    mocks.probeTarget.mockRejectedValueOnce(
      new Error('ssh handshake failed at internal transport stage'),
    );
    await act(async () => {
      root.render(
        <DispatchTargetDialog
          open
          target={{ kind: 'ssh', connectionId: 'ssh-2', displayName: 'backup-host' }}
          sourceWorkspacePath="/home/me/project"
          sourceWorkspaceId="workspace-source"
          onClose={vi.fn()}
          onReady={vi.fn()}
        />,
      );
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(container.textContent).toContain('dispatch.probeFailed');
    expect(container.textContent).not.toContain('internal transport stage');
  });

  it('explains the Git baseline and never offers a snapshot delivery mode', async () => {
    await act(async () => {
      root.render(
        <DispatchTargetDialog
          open
          target={{ kind: 'ssh', connectionId: 'ssh-1', displayName: 'build-host' }}
          sourceWorkspacePath="/home/me/project"
          sourceWorkspaceId="workspace-source"
          onClose={vi.fn()}
          onReady={vi.fn()}
        />,
      );
      await Promise.resolve();
    });

    expect(container.textContent).toContain('dispatch.baselineSource');
    expect(container.textContent).toContain('dispatch.baselineDescription');
    expect(container.textContent).toContain('dispatch.baseRefHint');
    expect(container.textContent).toContain('dispatch.includeUncommittedHint');
    expect(container.textContent).not.toContain('dispatch.deliverySnapshot');
    expect(container.textContent).not.toContain('dispatch.snapshotResultLocationHint');
  });

  it('carries the target model snapshot without asking about models or approval', async () => {
    const onReady = vi.fn();
    mocks.probeTarget.mockResolvedValue({
      cliInstalled: true,
      os: 'linux',
      arch: 'x86_64',
      installSupported: false,
      protocol: {
        productId: 'openbitfun',
        dataNamespace: 'openbitfun',
        protocolVersion: DISPATCH_PROTOCOL_VERSION,
        cliVersion: '1.2.3',
        os: 'linux',
        arch: 'x86_64',
        capabilities: READY_CAPABILITIES,
        modelConfigured: true,
        availableModels: ['model-a', 'model-b'],
        defaultModel: 'model-b',
      },
    });

    await act(async () => {
      root.render(
        <DispatchTargetDialog
          open
          target={{ kind: 'ssh', connectionId: 'ssh-1', displayName: 'build-host' }}
          sourceWorkspacePath="/home/me/project"
          sourceWorkspaceId="workspace-source"
          onClose={vi.fn()}
          onReady={onReady}
        />,
      );
      await Promise.resolve();
      await Promise.resolve();
    });

    // Both are ordinary composer controls, so setup neither shows nor asks.
    expect(container.textContent).not.toContain('dispatch.approval');
    expect(container.textContent).not.toContain('dispatch.modelStatus');
    expect(container.textContent).not.toContain('dispatch.syncModel');
    expect(container.querySelector('[role="radiogroup"]')).toBeNull();

    const useTarget = Array.from(container.querySelectorAll('button'))
      .find(button => button.textContent?.includes('dispatch.useTarget'));
    expect(
      useTarget?.disabled,
      'a ready target needs no further choice before it can be used',
    ).toBe(false);

    await act(async () => {
      useTarget?.click();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(onReady).toHaveBeenCalledWith(expect.objectContaining({
      includeUncommitted: false,
      availableModels: ['model-a', 'model-b'],
      defaultModel: 'model-b',
    }));
  });

  it('rejects a target that cannot serve every approval policy', async () => {
    // The policy is switched between turns, so a target that only supports the
    // one the user happens to start with would break on the next turn.
    mocks.probeTarget.mockResolvedValue({
      cliInstalled: true,
      os: 'linux',
      arch: 'x86_64',
      installSupported: false,
      protocol: {
        productId: 'openbitfun',
        dataNamespace: 'openbitfun',
        protocolVersion: DISPATCH_PROTOCOL_VERSION,
        cliVersion: '1.2.3',
        os: 'linux',
        arch: 'x86_64',
        capabilities: READY_CAPABILITIES.filter(
          capability => capability !== 'approval_remote',
        ),
        modelConfigured: true,
        availableModels: ['model-a'],
      },
    });

    await act(async () => {
      root.render(
        <DispatchTargetDialog
          open
          target={{ kind: 'ssh', connectionId: 'ssh-1', displayName: 'build-host' }}
          sourceWorkspacePath="/home/me/project"
          sourceWorkspaceId="workspace-source"
          onClose={vi.fn()}
          onReady={vi.fn()}
        />,
      );
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(container.textContent).toContain('dispatch.cliUpdateRequired');
    expect(container.textContent).not.toContain('approval_remote');
    const useTarget = Array.from(container.querySelectorAll('button'))
      .find(button => button.textContent?.includes('dispatch.useTarget'));
    expect(useTarget?.disabled).toBe(true);
  });

  it('accepts a target with no model configured at all', async () => {
    // Submission pushes this device's model configuration to a target that
    // cannot serve the choice, so an empty target is a setup step the backend
    // owns rather than a reason to refuse the target here.
    const onReady = vi.fn();
    mocks.probeTarget.mockResolvedValue({
      cliInstalled: true,
      os: 'linux',
      arch: 'x86_64',
      installSupported: false,
      protocol: {
        productId: 'openbitfun',
        dataNamespace: 'openbitfun',
        protocolVersion: DISPATCH_PROTOCOL_VERSION,
        cliVersion: '1.2.3',
        os: 'linux',
        arch: 'x86_64',
        capabilities: READY_CAPABILITIES,
        modelConfigured: false,
        availableModels: [],
      },
    });

    await act(async () => {
      root.render(
        <DispatchTargetDialog
          open
          target={{ kind: 'ssh', connectionId: 'ssh-1', displayName: 'build-host' }}
          sourceWorkspacePath="/home/me/project"
          sourceWorkspaceId="workspace-source"
          onClose={vi.fn()}
          onReady={onReady}
        />,
      );
      await Promise.resolve();
      await Promise.resolve();
    });

    const useTarget = Array.from(container.querySelectorAll('button'))
      .find(button => button.textContent?.includes('dispatch.useTarget'));
    expect(useTarget?.disabled).toBe(false);

    await act(async () => {
      useTarget?.click();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(onReady).toHaveBeenCalledTimes(1);
  });
});
