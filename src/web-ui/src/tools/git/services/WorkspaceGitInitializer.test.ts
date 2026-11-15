import { describe, expect, it, vi } from 'vitest';

const workspaceManagerMock = vi.hoisted(() => ({
  addEventListener: vi.fn(() => vi.fn()),
  getState: vi.fn(() => ({
    currentWorkspace: {
      id: 'workspace-1', rootPath: 'D:/workspace/OpenBitFun',
    },
  })),
}));

const gitStateManagerMock = vi.hoisted(() => ({
  refresh: vi.fn(async () => undefined),
  invalidateCache: vi.fn(),
}));

vi.mock('@/infrastructure/services/business/workspaceManager', () => ({
  workspaceManager: workspaceManagerMock,
}));

vi.mock('../state/GitStateManager', () => ({
  gitStateManager: gitStateManagerMock,
}));

import { workspaceGitInitializer } from './WorkspaceGitInitializer';

describe('WorkspaceGitInitializer startup refresh', () => {
  it('refreshes only the basic Git layer for the current workspace on startup', async () => {
    workspaceGitInitializer.stop();
    vi.clearAllMocks();
    workspaceGitInitializer.start();
    await Promise.resolve();

    expect(gitStateManagerMock.refresh).toHaveBeenCalledWith({ workspaceId: 'workspace-1' }, {
      layers: ['basic'],
      reason: 'mount',
      force: true,
      source: 'workspace_git_initializer',
    });
  });

  it('keeps outgoing workspace Git cache warm on switch and invalidates only on close', async () => {
    workspaceGitInitializer.stop();
    vi.clearAllMocks();
    let workspaceEventListener: ((event: any) => Promise<void> | void) | undefined;
    workspaceManagerMock.addEventListener.mockImplementationOnce((listener: any) => {
      workspaceEventListener = listener;
      return vi.fn();
    });

    workspaceGitInitializer.start();
    await Promise.resolve();
    gitStateManagerMock.refresh.mockClear();

    await workspaceEventListener?.({
      type: 'workspace:switched',
      workspace: { id: 'workspace-2', rootPath: 'D:/workspace/Other' },
    });

    expect(gitStateManagerMock.invalidateCache).not.toHaveBeenCalled();
    expect(gitStateManagerMock.refresh).toHaveBeenCalledWith({ workspaceId: 'workspace-2' }, {
      layers: ['basic'],
      reason: 'mount',
      force: false,
      source: 'workspace_git_initializer',
    });

    await workspaceEventListener?.({
      type: 'workspace:closed',
      workspaceId: 'workspace-1',
    });

    expect(gitStateManagerMock.invalidateCache).toHaveBeenCalledWith(
      { workspaceId: 'workspace-1' },
      ['basic', 'status', 'detailed'],
    );
  });
});
