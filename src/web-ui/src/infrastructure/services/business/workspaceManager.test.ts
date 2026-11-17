import { beforeEach, describe, expect, it, vi } from 'vitest';
import { isSurfaceChangedError } from '@/infrastructure/peer-device/deviceSurface';

const globalStateMocks = vi.hoisted(() => ({
  initializeWorkspaceStartupState: vi.fn(),
  cleanupInvalidWorkspaces: vi.fn(),
  getRecentWorkspaces: vi.fn(),
  getOpenedWorkspaces: vi.fn(),
  getAssistantWorkspaces: vi.fn(),
  openWorkspace: vi.fn(),
  openRemoteWorkspace: vi.fn(),
  openWorkspaceById: vi.fn(),
  reorderOpenedWorkspaces: vi.fn(),
  setActiveWorkspace: vi.fn(),
  closeWorkspace: vi.fn(),
  deleteAssistantWorkspace: vi.fn(),
  getCurrentWorkspace: vi.fn(),
  getPrimaryAssistantWorkspace: vi.fn(),
  updateWorkspaceInfo: vi.fn(),
  scanWorkspaceInfo: vi.fn(),
}));

const listenMock = vi.hoisted(() => vi.fn());
const flowChatMocks = vi.hoisted(() => ({
  cancelRunningSessionsForWorkspace: vi.fn().mockResolvedValue([]),
  removeSessionsForWorkspace: vi.fn(),
}));

vi.mock('@/flow_chat/store/FlowChatStore', () => ({ flowChatStore: flowChatMocks }));

vi.mock('../../../shared/types', () => ({
  WorkspaceKind: {
    Normal: 'normal',
    Assistant: 'assistant',
    Remote: 'remote',
  },
  globalStateAPI: globalStateMocks,
  isRemoteWorkspace: (workspace: { workspaceKind?: string } | null) =>
    workspace?.workspaceKind === 'remote',
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: listenMock,
}));

vi.mock('@/shared/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

vi.mock('@/shared/utils/startupTrace', () => ({
  startupTrace: {
    markPhase: vi.fn(),
  },
}));

function configureGlobalState(): void {
  globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
    cleanupRemovedCount: 0,
    recentWorkspaces: [],
    openedWorkspaces: [],
    currentWorkspace: null,
    legacyRemoteWorkspace: null,
  });
  globalStateMocks.cleanupInvalidWorkspaces.mockResolvedValue(0);
  globalStateMocks.getRecentWorkspaces.mockResolvedValue([]);
  globalStateMocks.getOpenedWorkspaces.mockResolvedValue([]);
  globalStateMocks.getAssistantWorkspaces.mockResolvedValue([]);
  flowChatMocks.cancelRunningSessionsForWorkspace.mockResolvedValue([]);
  globalStateMocks.getCurrentWorkspace.mockResolvedValue(null);
  globalStateMocks.getPrimaryAssistantWorkspace.mockResolvedValue(null);
  globalStateMocks.updateWorkspaceInfo.mockReset();
  globalStateMocks.scanWorkspaceInfo.mockReset();
}

async function getFreshWorkspaceManager() {
  const { manager } = await getFreshWorkspaceHarness();
  return manager;
}

async function getFreshWorkspaceHarness() {
  vi.resetModules();
  const deviceSurface = await import('@/infrastructure/peer-device/deviceSurface');
  deviceSurface.resetDeviceSurfaceForTest();
  const displayOrder = await import('./workspaceDisplayOrder');
  displayOrder.resetWorkspaceDisplayOrdersForTest();
  const { WorkspaceManager } = await import('./workspaceManager');
  (WorkspaceManager as unknown as { instance: unknown }).instance = null;
  return {
    manager: WorkspaceManager.getInstance(),
    deviceSurface,
    displayOrder,
  };
}

async function flushAsyncWork(): Promise<void> {
  await new Promise(resolve => setTimeout(resolve, 0));
}

describe('WorkspaceManager startup initialization', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    configureGlobalState();
  });

  it('does not block startup workspace state on identity listener registration', async () => {
    listenMock.mockReturnValue(new Promise(() => undefined));
    const manager = await getFreshWorkspaceManager();

    const initializePromise = manager.initialize();
    const initializeResult = await Promise.race([
      initializePromise.then(() => 'initialized'),
      new Promise(resolve => setTimeout(() => resolve('timeout'), 20)),
    ]);

    expect(listenMock).toHaveBeenCalledWith('workspace-identity-changed', expect.any(Function));
    expect(initializeResult).toBe('initialized');
    expect(globalStateMocks.initializeWorkspaceStartupState).toHaveBeenCalledTimes(1);
    expect(globalStateMocks.cleanupInvalidWorkspaces).not.toHaveBeenCalled();
    expect(globalStateMocks.getCurrentWorkspace).not.toHaveBeenCalled();
    expect(globalStateMocks.getRecentWorkspaces).not.toHaveBeenCalled();
    expect(globalStateMocks.getOpenedWorkspaces).not.toHaveBeenCalled();
  });

  it('applies identity updates after delayed listener registration completes', async () => {
    const workspace = {
      id: 'assistant-1',
      name: 'Assistant 1',
      rootPath: 'D:/workspace/assistant-1',
      workspaceKind: 'assistant',
      identity: null,
    };
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0,
      recentWorkspaces: [workspace],
      openedWorkspaces: [workspace],
      currentWorkspace: workspace,
      legacyRemoteWorkspace: null,
    });
    globalStateMocks.getCurrentWorkspace.mockResolvedValue(workspace);
    globalStateMocks.getRecentWorkspaces.mockResolvedValue([workspace]);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([workspace]);

    let identityHandler:
      | ((event: {
          payload: {
            workspaceId: string;
            workspacePath: string;
            name: string;
            identity: { name: string };
            changedFields: string[];
          };
        }) => void)
      | null = null;
    let resolveListener: ((unlisten: () => void) => void) | null = null;
    listenMock.mockImplementation((eventName, handler) => {
      if (eventName !== 'workspace-identity-changed') {
        return Promise.resolve(() => undefined);
      }
      identityHandler = handler;
      return new Promise(resolve => {
        resolveListener = resolve;
      });
    });

    const manager = await getFreshWorkspaceManager();
    await manager.initialize();

    expect(manager.getState().currentWorkspace?.name).toBe('Assistant 1');

    resolveListener?.(() => undefined);
    await flushAsyncWork();

    identityHandler?.({
      payload: {
        workspaceId: 'assistant-1',
        workspacePath: 'D:/workspace/assistant-1',
        name: 'Assistant renamed',
        identity: { name: 'Assistant renamed' },
        changedFields: ['name'],
      },
    });

    expect(manager.getState().currentWorkspace?.name).toBe('Assistant renamed');
    identityHandler?.({ payload: {
      workspaceId: 'another-workspace', workspacePath: workspace.rootPath,
      name: 'Wrong workspace', identity: { name: 'Wrong workspace' }, changedFields: ['name'],
    } });
    expect(manager.getState().currentWorkspace?.name).toBe('Assistant renamed');

  });

  it('refreshes workspace identity once the delayed listener is ready after startup', async () => {
    const startupWorkspace = {
      id: 'assistant-1',
      name: 'Assistant 1',
      rootPath: 'D:/workspace/assistant-1',
      workspaceKind: 'assistant',
      identity: null,
    };
    const refreshedWorkspace = {
      ...startupWorkspace,
      name: 'Assistant renamed',
      identity: { name: 'Assistant renamed' },
    };
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0,
      recentWorkspaces: [startupWorkspace],
      openedWorkspaces: [startupWorkspace],
      currentWorkspace: startupWorkspace,
      legacyRemoteWorkspace: null,
    });
    globalStateMocks.getCurrentWorkspace.mockResolvedValue(refreshedWorkspace);
    globalStateMocks.getRecentWorkspaces.mockResolvedValue([refreshedWorkspace]);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([refreshedWorkspace]);

    let resolveListener: ((unlisten: () => void) => void) | null = null;
    listenMock.mockImplementation(eventName => eventName === 'workspace-identity-changed'
      ? new Promise<() => void>(resolve => {
        resolveListener = resolve;
      })
      : Promise.resolve(() => undefined));

    const manager = await getFreshWorkspaceManager();
    await manager.initialize();

    expect(manager.getState().currentWorkspace?.name).toBe('Assistant 1');

    resolveListener?.(() => undefined);
    await flushAsyncWork();

    expect(globalStateMocks.getCurrentWorkspace).toHaveBeenCalledTimes(1);
    expect(manager.getState().currentWorkspace?.name).toBe('Assistant renamed');
  });

  it('refreshes workspace identity when an identity event arrives before startup state is committed', async () => {
    const startupWorkspace = {
      id: 'assistant-1',
      name: 'Assistant 1',
      rootPath: 'D:/workspace/assistant-1',
      workspaceKind: 'assistant',
      identity: null,
    };
    const refreshedWorkspace = {
      ...startupWorkspace,
      name: 'Assistant renamed',
      identity: { name: 'Assistant renamed' },
    };
    let resolveStartupState: ((value: {
      cleanupRemovedCount: number;
      recentWorkspaces: typeof startupWorkspace[];
      openedWorkspaces: typeof startupWorkspace[];
      currentWorkspace: typeof startupWorkspace;
      legacyRemoteWorkspace: null;
    }) => void) | null = null;
    globalStateMocks.initializeWorkspaceStartupState.mockReturnValue(new Promise(resolve => {
      resolveStartupState = resolve;
    }));
    globalStateMocks.getCurrentWorkspace.mockResolvedValue(refreshedWorkspace);
    globalStateMocks.getRecentWorkspaces.mockResolvedValue([refreshedWorkspace]);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([refreshedWorkspace]);

    let identityHandler:
      | ((event: {
          payload: {
            workspaceId: string;
            workspacePath: string;
            name: string;
            identity: { name: string };
            changedFields: string[];
          };
        }) => void)
      | null = null;
    listenMock.mockImplementation((eventName, handler) => {
      if (eventName === 'workspace-identity-changed') {
        identityHandler = handler;
      }
      return Promise.resolve(() => undefined);
    });

    const manager = await getFreshWorkspaceManager();
    const initializePromise = manager.initialize();
    await flushAsyncWork();

    identityHandler?.({
      payload: {
        workspaceId: 'assistant-1',
        workspacePath: 'D:/workspace/assistant-1',
        name: 'Assistant renamed',
        identity: { name: 'Assistant renamed' },
        changedFields: ['name'],
      },
    });

    resolveStartupState?.({
      cleanupRemovedCount: 0,
      recentWorkspaces: [startupWorkspace],
      openedWorkspaces: [startupWorkspace],
      currentWorkspace: startupWorkspace,
      legacyRemoteWorkspace: null,
    });
    await initializePromise;
    await flushAsyncWork();

    expect(globalStateMocks.getCurrentWorkspace).toHaveBeenCalledTimes(1);
    expect(manager.getState().currentWorkspace?.name).toBe('Assistant renamed');
  });

  it('keeps startup workspace state available when identity listener registration fails', async () => {
    listenMock.mockRejectedValue(new Error('listener unavailable'));
    const manager = await getFreshWorkspaceManager();

    await expect(manager.initialize()).resolves.toBeUndefined();

    expect(globalStateMocks.initializeWorkspaceStartupState).toHaveBeenCalledTimes(1);
    expect(manager.getState().loading).toBe(false);
    expect(manager.getState().error).toBeNull();
  });

  it('keeps startup workspace state available when identity listener registration throws synchronously', async () => {
    listenMock.mockImplementation(() => {
      throw new Error('listener unavailable');
    });
    const manager = await getFreshWorkspaceManager();

    await expect(manager.initialize()).resolves.toBeUndefined();

    expect(globalStateMocks.initializeWorkspaceStartupState).toHaveBeenCalledTimes(1);
    expect(manager.getState().loading).toBe(false);
    expect(manager.getState().error).toBeNull();
  });

  it('fails a Peer-mode rebootstrap instead of leaving workspace loading unresolved', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockRejectedValue(
      new Error('peer request timed out'),
    );
    listenMock.mockResolvedValue(() => undefined);
    const manager = await getFreshWorkspaceManager();

    await expect(manager.reinitializeForPeerModeSwitch()).rejects.toThrow(
      'peer request timed out',
    );

    expect(manager.getState()).toMatchObject({
      loading: false,
      error: 'peer request timed out',
    });
  });

  it('stores the startup legacy remote workspace snapshot for one reconnect pass', async () => {
    const legacyRemoteWorkspace = {
      connectionId: 'conn-1',
      connectionName: 'Remote',
      remotePath: '/repo',
      sshHost: 'devbox',
    };
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0,
      recentWorkspaces: [],
      openedWorkspaces: [],
      currentWorkspace: null,
      legacyRemoteWorkspace,
    });
    listenMock.mockResolvedValue(() => undefined);
    const manager = await getFreshWorkspaceManager();

    await manager.initialize();

    expect(manager.consumeStartupLegacyRemoteWorkspaceSnapshot()).toEqual({
      available: true,
      workspace: legacyRemoteWorkspace,
    });
    expect(manager.consumeStartupLegacyRemoteWorkspaceSnapshot()).toEqual({
      available: false,
      workspace: null,
    });
  });
});

describe('WorkspaceManager assistant closing and reopening', () => {
  const assistant = {
    id: 'assistant-closed', name: 'Assistant', rootPath: '/test/assistants/closed',
    workspaceKind: 'assistant',
  };
  beforeEach(() => {
    vi.clearAllMocks();
    configureGlobalState();
    listenMock.mockResolvedValue(() => undefined);
  });

  it('keeps closed assistants in the catalog and reopens the same record by ID', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0, currentWorkspace: null, openedWorkspaces: [],
      recentWorkspaces: [], assistantWorkspaces: [assistant], primaryAssistantWorkspaceId: assistant.id,
    });
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();
    expect(manager.getState().openedWorkspaces.size).toBe(0);
    expect(manager.getState().assistantWorkspaces).toEqual([assistant]);

    globalStateMocks.openWorkspaceById.mockResolvedValue(assistant);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([assistant]);
    await manager.setActiveWorkspace(assistant.id);
    expect(globalStateMocks.openWorkspaceById).toHaveBeenCalledWith(assistant.id);
    expect(globalStateMocks.setActiveWorkspace).not.toHaveBeenCalled();
    expect(manager.getState().activeWorkspaceId).toBe(assistant.id);
    expect(manager.getState().openedWorkspaces.has(assistant.id)).toBe(true);
    expect(manager.getState().assistantWorkspaces).toHaveLength(1);
  });

  it('shows a switched workspace after the host switch alone, refreshing lists afterwards', async () => {
    const first = { id: 'first', name: 'First', rootPath: '/test/first', workspaceKind: 'normal' };
    const second = { id: 'second', name: 'Second', rootPath: '/test/second', workspaceKind: 'normal' };
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0, currentWorkspace: first, openedWorkspaces: [first, second],
      recentWorkspaces: [], assistantWorkspaces: [], primaryAssistantWorkspaceId: null,
    });
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();
    let releaseLists!: (value: unknown) => void;
    globalStateMocks.getOpenedWorkspaces.mockReturnValue(new Promise(resolve => { releaseLists = resolve; }));
    globalStateMocks.setActiveWorkspace.mockResolvedValue(second);
    await manager.setActiveWorkspace(second.id);
    // Resolved while the list refresh is still in flight.
    expect(manager.getState().activeWorkspaceId).toBe(second.id);
    expect([...manager.getState().openedWorkspaces.keys()]).toEqual(['first', 'second']);
    releaseLists([first, second]);
  });

  it('waits for running sessions to be cancelled before closing and preserves the catalog', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0, currentWorkspace: assistant, openedWorkspaces: [assistant],
      recentWorkspaces: [], assistantWorkspaces: [assistant], primaryAssistantWorkspaceId: assistant.id,
    });
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();
    let releaseCancellation!: (ids: string[]) => void;
    flowChatMocks.cancelRunningSessionsForWorkspace.mockReturnValueOnce(new Promise(resolve => {
      releaseCancellation = resolve;
    }));
    const closing = manager.closeWorkspaceById(assistant.id);
    await flushAsyncWork();
    expect(flowChatMocks.cancelRunningSessionsForWorkspace).toHaveBeenCalledWith(assistant);
    expect(globalStateMocks.closeWorkspace).not.toHaveBeenCalled();
    releaseCancellation(['session-running']);
    await closing;
    expect(globalStateMocks.closeWorkspace).toHaveBeenCalledWith(assistant.id);
    expect(manager.getState().openedWorkspaces.size).toBe(0);
    expect(manager.getState().currentWorkspace).toBeNull();
    expect(manager.getState().assistantWorkspaces).toEqual([assistant]);
    expect(manager.getState().primaryAssistantWorkspaceId).toBe(assistant.id);
    expect(flowChatMocks.removeSessionsForWorkspace).not.toHaveBeenCalled();
  });

  it('keeps older hosts on the existing opened-only catalog', async () => {
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();
    expect(manager.getState().assistantWorkspaces).toBeNull();
    expect(globalStateMocks.getAssistantWorkspaces).not.toHaveBeenCalled();
  });

  it('does not close a different host after switching surfaces during cancellation', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0, currentWorkspace: assistant, openedWorkspaces: [assistant],
      recentWorkspaces: [], assistantWorkspaces: [assistant],
    });
    const { manager, deviceSurface } = await getFreshWorkspaceHarness();
    await manager.initialize();
    let releaseCancellation!: (ids: string[]) => void;
    flowChatMocks.cancelRunningSessionsForWorkspace.mockReturnValueOnce(new Promise(resolve => {
      releaseCancellation = resolve;
    }));
    const closing = manager.closeWorkspaceById(assistant.id);
    await flushAsyncWork();
    deviceSurface.activateSurface('peer-b');
    releaseCancellation(['session-running']);
    await expect(closing).rejects.toSatisfy(isSurfaceChangedError);
    expect(globalStateMocks.closeWorkspace).not.toHaveBeenCalled();
    expect(manager.getState().error).toBeNull();
  });

  it('removes a closed assistant from the full catalog when explicitly deleted', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0, currentWorkspace: null, openedWorkspaces: [],
      recentWorkspaces: [], assistantWorkspaces: [assistant],
    });
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();
    await manager.deleteAssistantWorkspace(assistant.id);
    expect(manager.getState().assistantWorkspaces).toEqual([]);
    expect(flowChatMocks.removeSessionsForWorkspace).toHaveBeenCalledWith(assistant);
  });

  it('keeps assistant catalogs isolated across local and peer device surfaces', async () => {
    const { manager, deviceSurface } = await getFreshWorkspaceHarness();
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValueOnce({
      cleanupRemovedCount: 0, currentWorkspace: null, openedWorkspaces: [],
      recentWorkspaces: [], assistantWorkspaces: [assistant],
    });
    await manager.initialize();
    deviceSurface.activateSurface('peer-b');
    expect(manager.getState().assistantWorkspaces).toBeNull();
    const peerAssistant = { ...assistant, name: 'Peer assistant' };
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValueOnce({
      cleanupRemovedCount: 0, currentWorkspace: peerAssistant, openedWorkspaces: [peerAssistant],
      recentWorkspaces: [], assistantWorkspaces: [peerAssistant],
    });
    await manager.initialize();
    expect(manager.getState().assistantWorkspaces).toEqual([peerAssistant]);
    deviceSurface.activateSurface(deviceSurface.LOCAL_SURFACE_ID);
    expect(manager.getState().assistantWorkspaces).toEqual([assistant]);
    expect(manager.getState().openedWorkspaces.size).toBe(0);
  });

  it('updates a closed assistant configuration without reopening it', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0, currentWorkspace: null, openedWorkspaces: [],
      recentWorkspaces: [], assistantWorkspaces: [assistant],
    });
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();
    const renamed = { ...assistant, name: 'Renamed assistant' };
    globalStateMocks.updateWorkspaceInfo.mockResolvedValueOnce(renamed);
    await manager.renameWorkspace(assistant.id, renamed.name);
    expect(manager.getState().assistantWorkspaces).toEqual([renamed]);
    expect(manager.getState().openedWorkspaces.size).toBe(0);
    expect(manager.getState().activeWorkspaceId).toBeNull();
  });
});

describe('WorkspaceManager device surface switching', () => {
  const workspaceA = {
    id: 'workspace-a',
    name: 'Device A repo',
    rootPath: '/repo',
    workspaceKind: 'normal',
    identity: null,
  };
  const workspaceB = {
    ...workspaceA,
    id: 'workspace-b',
    name: 'Device B repo',
  };

  function startupSnapshot(workspace: typeof workspaceA) {
    return {
      cleanupRemovedCount: 0,
      recentWorkspaces: [workspace],
      openedWorkspaces: [workspace],
      currentWorkspace: workspace,
      primaryAssistantWorkspaceId: null,
      legacyRemoteWorkspace: null,
    };
  }

  beforeEach(() => {
    vi.clearAllMocks();
    configureGlobalState();
    listenMock.mockResolvedValue(() => undefined);
  });

  it('lets a later switch supersede an in-flight rebootstrap instead of running two', async () => {
    const startupResolvers: Array<(snapshot: ReturnType<typeof startupSnapshot>) => void> = [];
    globalStateMocks.initializeWorkspaceStartupState.mockImplementation(
      () => new Promise(resolve => {
        startupResolvers.push(resolve);
      }),
    );

    const manager = await getFreshWorkspaceManager();

    const first = manager.reinitializeForPeerModeSwitch().then(
      () => null,
      (error: unknown) => error,
    );
    await flushAsyncWork();
    const second = manager.reinitializeForPeerModeSwitch().then(
      () => null,
      (error: unknown) => error,
    );
    await flushAsyncWork();

    expect(startupResolvers).toHaveLength(2);

    // The switch that superseded the first one lands first; the abandoned load
    // then answers late, which is exactly when it used to overwrite state.
    startupResolvers[1](startupSnapshot(workspaceB));
    await flushAsyncWork();
    startupResolvers[0](startupSnapshot(workspaceA));

    expect(isSurfaceChangedError(await first)).toBe(true);
    expect(await second).toBeNull();

    const state = manager.getState();
    expect(state.currentWorkspace?.id).toBe('workspace-b');
    expect(Array.from(state.openedWorkspaces.keys())).toEqual(['workspace-b']);
    expect(state.error).toBeNull();
    expect(state.loading).toBe(false);
  });

  it('makes a concurrent initialize join the in-flight load instead of resolving early', async () => {
    let resolveStartup: ((snapshot: ReturnType<typeof startupSnapshot>) => void) | null = null;
    globalStateMocks.initializeWorkspaceStartupState.mockReturnValue(
      new Promise(resolve => {
        resolveStartup = resolve;
      }),
    );

    const manager = await getFreshWorkspaceManager();

    const first = manager.initialize();
    let joinedSettled = false;
    const joined = manager.initialize().then(() => {
      joinedSettled = true;
    });
    await flushAsyncWork();

    expect(globalStateMocks.initializeWorkspaceStartupState).toHaveBeenCalledTimes(1);
    expect(joinedSettled).toBe(false);

    resolveStartup?.(startupSnapshot(workspaceA));
    await Promise.all([first, joined]);

    expect(joinedSettled).toBe(true);
    expect(manager.getState().currentWorkspace?.id).toBe('workspace-a');
  });

  it('keeps a superseded load from reporting its failure onto the new surface', async () => {
    const startupOutcomes: Array<{
      resolve: (snapshot: ReturnType<typeof startupSnapshot>) => void;
      reject: (error: Error) => void;
    }> = [];
    globalStateMocks.initializeWorkspaceStartupState.mockImplementation(
      () => new Promise((resolve, reject) => {
        startupOutcomes.push({ resolve, reject });
      }),
    );

    const manager = await getFreshWorkspaceManager();

    const first = manager.reinitializeForPeerModeSwitch().then(
      () => null,
      (error: unknown) => error,
    );
    await flushAsyncWork();
    const second = manager.reinitializeForPeerModeSwitch().then(
      () => null,
      (error: unknown) => error,
    );
    await flushAsyncWork();

    startupOutcomes[1].resolve(startupSnapshot(workspaceB));
    await flushAsyncWork();
    startupOutcomes[0].reject(new Error('peer request timed out'));

    expect(isSurfaceChangedError(await first)).toBe(true);
    expect(await second).toBeNull();
    expect(manager.getState().error).toBeNull();
    expect(manager.getState().currentWorkspace?.id).toBe('workspace-b');
  });

  it('selects each device cached workspace state before its refresh completes', async () => {
    globalStateMocks.initializeWorkspaceStartupState
      .mockResolvedValueOnce(startupSnapshot(workspaceA))
      .mockResolvedValueOnce(startupSnapshot(workspaceB));
    const { manager, deviceSurface } = await getFreshWorkspaceHarness();

    await manager.initialize();
    expect(manager.getState().currentWorkspace?.id).toBe('workspace-a');

    deviceSurface.activateSurface('peer-b');
    expect(manager.getState().currentWorkspace).toBeNull();
    await manager.reinitializeForPeerModeSwitch();
    expect(manager.getState().currentWorkspace?.id).toBe('workspace-b');

    deviceSurface.activateSurface(deviceSurface.LOCAL_SURFACE_ID);
    expect(manager.getState().currentWorkspace?.id).toBe('workspace-a');
    deviceSurface.activateSurface('peer-b');
    expect(manager.getState().currentWorkspace?.id).toBe('workspace-b');
  });

  it('shows a device in the order its background catalog settled on, live and cached alike', async () => {
    const workspace = (id: string) => ({ ...workspaceA, id, name: id, rootPath: `/${id}` });
    const [a, b, c] = [workspace('a'), workspace('b'), workspace('c')];
    globalStateMocks.initializeWorkspaceStartupState
      .mockResolvedValueOnce({ ...startupSnapshot(a), openedWorkspaces: [a, b, c] })
      .mockResolvedValueOnce(startupSnapshot(workspaceB));
    const { manager, deviceSurface } = await getFreshWorkspaceHarness();
    const displayOrder = await import('./workspaceDisplayOrder');
    await manager.initialize();
    deviceSurface.activateSurface('peer-b');
    await manager.reinitializeForPeerModeSwitch();

    // While another device is rendered, this device's catalog reads a list
    // arranged on another controller.
    displayOrder.setWorkspaceDisplayOrder(deviceSurface.LOCAL_SURFACE_ID, ['c', 'b', 'a']);
    // The background section's fallback and the live list returned to both
    // draw that order; the live list's next host read keeps it.
    expect(manager.getSurfaceOpenedWorkspaces(deviceSurface.LOCAL_SURFACE_ID).map(item => item.id)).toEqual(['c', 'b', 'a']);
    deviceSurface.activateSurface(deviceSurface.LOCAL_SURFACE_ID);
    expect([...manager.getState().openedWorkspaces.keys()]).toEqual(['c', 'b', 'a']);
    globalStateMocks.setActiveWorkspace.mockResolvedValue(b);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([c, b, a]);
    await manager.setActiveWorkspace(b.id);
    await flushAsyncWork();
    expect([...manager.getState().openedWorkspaces.keys()]).toEqual(['c', 'b', 'a']);
  });

  it('keeps a cached surface visible when its refresh fails', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue(
      startupSnapshot(workspaceA),
    );
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();

    globalStateMocks.initializeWorkspaceStartupState.mockRejectedValue(
      new Error('relay unavailable'),
    );
    await expect(manager.reinitializeForPeerModeSwitch()).rejects.toThrow('relay unavailable');

    expect(manager.getState()).toMatchObject({
      currentWorkspace: workspaceA,
      loading: false,
      error: 'relay unavailable',
    });
  });

  it('keeps local identity events out of a rendered peer with equal workspace ids', async () => {
    const localWorkspace = {
      id: 'shared-workspace-id',
      name: 'Local repo',
      rootPath: '/repo',
      workspaceKind: 'normal',
      identity: null,
    };
    const peerWorkspace = {
      ...localWorkspace,
      name: 'Peer repo',
    };
    globalStateMocks.initializeWorkspaceStartupState
      .mockResolvedValueOnce(startupSnapshot(localWorkspace))
      .mockResolvedValueOnce(startupSnapshot(peerWorkspace));
    let identityHandler: ((event: { payload: {
      workspaceId: string;
      workspacePath: string;
      name: string;
      identity: { name: string };
      changedFields: string[];
    } }) => void) | null = null;
    listenMock.mockImplementation((eventName, handler) => {
      if (eventName === 'workspace-identity-changed') {
        identityHandler = handler;
      }
      return Promise.resolve(() => undefined);
    });
    const { manager, deviceSurface } = await getFreshWorkspaceHarness();

    await manager.initialize();
    deviceSurface.activateSurface('peer-b');
    await manager.reinitializeForPeerModeSwitch();

    identityHandler?.({
      payload: {
        workspaceId: 'shared-workspace-id',
        workspacePath: '/repo',
        name: 'Local repo renamed',
        identity: { name: 'Local repo renamed' },
        changedFields: ['name'],
      },
    });

    expect(manager.getState().currentWorkspace?.name).toBe('Peer repo');
    deviceSurface.activateSurface(deviceSurface.LOCAL_SURFACE_ID);
    expect(manager.getState().currentWorkspace?.name).toBe('Local repo renamed');
  });
});

describe('WorkspaceManager project rename', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    configureGlobalState();
  });

  it('updates current, opened, and recent workspace records together', async () => {
    const workspace = {
      id: 'project-1',
      name: 'Original project',
      rootPath: 'D:/workspace/project-1',
      workspaceKind: 'normal',
    };
    const renamedWorkspace = {
      ...workspace,
      name: 'Renamed project',
    };
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0,
      recentWorkspaces: [workspace],
      openedWorkspaces: [workspace],
      currentWorkspace: workspace,
      legacyRemoteWorkspace: null,
    });
    globalStateMocks.updateWorkspaceInfo.mockResolvedValue(renamedWorkspace);
    listenMock.mockResolvedValue(() => undefined);

    const manager = await getFreshWorkspaceManager();
    await manager.initialize();

    await expect(manager.renameWorkspace('project-1', '  Renamed project  '))
      .resolves.toEqual(renamedWorkspace);

    expect(globalStateMocks.updateWorkspaceInfo).toHaveBeenCalledWith('project-1', {
      name: 'Renamed project',
    });
    const state = manager.getState();
    expect(state.currentWorkspace?.name).toBe('Renamed project');
    expect(state.openedWorkspaces.get('project-1')?.name).toBe('Renamed project');
    expect(state.recentWorkspaces[0]?.name).toBe('Renamed project');
  });

  it('publishes a project name and folder associations in the same record update', async () => {
    const workspace = { id: 'project-1', name: 'frontend', rootPath: '/srv/frontend', workspaceKind: 'normal' };
    const relatedPaths = [{ path: '/srv/backend' }];
    const updated = { ...workspace, name: 'Product', relatedPaths };
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0, currentWorkspace: workspace,
      openedWorkspaces: [workspace], recentWorkspaces: [workspace], legacyRemoteWorkspace: null,
    });
    globalStateMocks.updateWorkspaceInfo.mockResolvedValue(updated);
    listenMock.mockResolvedValue(() => undefined);
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();
    await manager.updateWorkspaceInfo(workspace.id, { name: 'Product', relatedPaths });
    expect(globalStateMocks.updateWorkspaceInfo).toHaveBeenCalledOnce();
    expect(globalStateMocks.updateWorkspaceInfo).toHaveBeenCalledWith(workspace.id, { name: 'Product', relatedPaths });
    expect(manager.getState().currentWorkspace).toMatchObject(updated);
    expect(manager.getState().openedWorkspaces.get(workspace.id)).toMatchObject(updated);
    expect(manager.getState().recentWorkspaces[0]).toMatchObject(updated);
  });

});

describe('WorkspaceManager host catalog hints', () => {
  const localWorkspace = {
    id: 'workspace-local',
    name: 'Local repo',
    rootPath: '/repo/local',
    workspaceKind: 'normal',
    identity: null,
  };
  const remoteOpenedWorkspace = {
    id: 'workspace-remote-opened',
    name: 'Opened from phone',
    rootPath: '/repo/phone',
    workspaceKind: 'normal',
    identity: null,
  };

  type CatalogHandler = (event: { payload: Record<string, unknown> }) => void;

  function captureCatalogHandler(): { current: () => CatalogHandler | null } {
    let handler: CatalogHandler | null = null;
    listenMock.mockImplementation((eventName, callback) => {
      if (eventName === 'workspace-catalog-changed') {
        handler = callback;
      }
      return Promise.resolve(() => undefined);
    });
    return { current: () => handler };
  }

  function snapshot(workspaces: Array<typeof localWorkspace>, current: typeof localWorkspace | null) {
    return {
      cleanupRemovedCount: 0,
      recentWorkspaces: workspaces,
      openedWorkspaces: workspaces,
      currentWorkspace: current,
      primaryAssistantWorkspaceId: null,
      legacyRemoteWorkspace: null,
    };
  }

  it('refreshes closed assistants from the host catalog without adding them to the sidebar', async () => {
    const assistant = { ...localWorkspace, id: 'closed-assistant', workspaceKind: 'assistant' };
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      ...snapshot([], null), assistantWorkspaces: [assistant],
    });
    const handler = captureCatalogHandler();
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();
    const renamed = { ...assistant, name: 'Updated on host' };
    globalStateMocks.getAssistantWorkspaces.mockResolvedValue([renamed]);
    handler.current()?.({ payload: { revision: 1 } });
    await flushAsyncWork();
    expect(manager.getState().assistantWorkspaces).toEqual([renamed]);
    expect(manager.getState().openedWorkspaces.size).toBe(0);
    globalStateMocks.getAssistantWorkspaces.mockResolvedValue([]);
    handler.current()?.({ payload: { revision: 2 } });
    await flushAsyncWork();
    expect(manager.getState().assistantWorkspaces).toEqual([]);
  });

  beforeEach(() => {
    vi.clearAllMocks();
    configureGlobalState();
  });

  it('subscribes to host catalog hints during startup', async () => {
    listenMock.mockResolvedValue(() => undefined);
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();

    expect(listenMock).toHaveBeenCalledWith('workspace-catalog-changed', expect.any(Function));
  });

  it('shows a workspace another controller opened without stealing the active slot', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue(
      snapshot([localWorkspace], localWorkspace),
    );
    const handler = captureCatalogHandler();
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();
    const events: string[] = [];
    manager.addEventListener(event => {
      events.push(event.type);
    });

    // The remote controller selected its workspace, so the host's current
    // workspace moved; this surface keeps what its user is looking at.
    globalStateMocks.getCurrentWorkspace.mockResolvedValue(remoteOpenedWorkspace);
    globalStateMocks.getRecentWorkspaces.mockResolvedValue([remoteOpenedWorkspace, localWorkspace]);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([localWorkspace, remoteOpenedWorkspace]);

    handler.current()?.({ payload: { revision: 3 } });
    await flushAsyncWork();

    const state = manager.getState();
    expect(globalStateMocks.getOpenedWorkspaces).toHaveBeenCalledTimes(1);
    expect(Array.from(state.openedWorkspaces.keys())).toEqual([
      'workspace-local',
      'workspace-remote-opened',
    ]);
    expect(state.activeWorkspaceId).toBe('workspace-local');
    expect(state.currentWorkspace?.id).toBe('workspace-local');
    expect(state.recentWorkspaces.map(workspace => workspace.id)).toEqual([
      'workspace-remote-opened',
      'workspace-local',
    ]);
    expect(events).toEqual(['workspace:opened']);
  });

  it('adopts the host current workspace when this surface had no selection', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue(snapshot([], null));
    const handler = captureCatalogHandler();
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();
    const events: string[] = [];
    manager.addEventListener(event => {
      events.push(event.type);
    });

    globalStateMocks.getCurrentWorkspace.mockResolvedValue(remoteOpenedWorkspace);
    globalStateMocks.getRecentWorkspaces.mockResolvedValue([remoteOpenedWorkspace]);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([remoteOpenedWorkspace]);

    handler.current()?.({ payload: { revision: 1 } });
    await flushAsyncWork();

    expect(manager.getState().activeWorkspaceId).toBe('workspace-remote-opened');
    expect(events).toEqual(['workspace:opened', 'workspace:active-changed']);
  });

  it('retires a workspace the host closed and moves the selection with the host', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue(
      snapshot([localWorkspace, remoteOpenedWorkspace], remoteOpenedWorkspace),
    );
    const handler = captureCatalogHandler();
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();
    const events: string[] = [];
    manager.addEventListener(event => {
      events.push(event.type);
    });

    globalStateMocks.getCurrentWorkspace.mockResolvedValue(localWorkspace);
    globalStateMocks.getRecentWorkspaces.mockResolvedValue([localWorkspace]);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([localWorkspace]);

    handler.current()?.({ payload: { revision: 2 } });
    await flushAsyncWork();

    const state = manager.getState();
    expect(Array.from(state.openedWorkspaces.keys())).toEqual(['workspace-local']);
    expect(state.activeWorkspaceId).toBe('workspace-local');
    expect(events).toEqual(['workspace:closed', 'workspace:active-changed']);
  });

  it('coalesces hints that arrive while a re-read is running into one more re-read', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue(
      snapshot([localWorkspace], localWorkspace),
    );
    const handler = captureCatalogHandler();
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();

    let releaseFirstRead: ((workspaces: Array<typeof localWorkspace>) => void) | null = null;
    globalStateMocks.getOpenedWorkspaces
      .mockImplementationOnce(() => new Promise(resolve => {
        releaseFirstRead = resolve;
      }))
      .mockResolvedValue([localWorkspace, remoteOpenedWorkspace]);
    globalStateMocks.getCurrentWorkspace.mockResolvedValue(localWorkspace);
    globalStateMocks.getRecentWorkspaces.mockResolvedValue([localWorkspace, remoteOpenedWorkspace]);

    handler.current()?.({ payload: { revision: 4 } });
    await flushAsyncWork();
    handler.current()?.({ payload: { revision: 5 } });
    handler.current()?.({ payload: { revision: 6 } });
    await flushAsyncWork();
    expect(globalStateMocks.getOpenedWorkspaces).toHaveBeenCalledTimes(1);

    releaseFirstRead?.([localWorkspace]);
    await flushAsyncWork();
    await flushAsyncWork();

    expect(globalStateMocks.getOpenedWorkspaces).toHaveBeenCalledTimes(2);
    expect(manager.getState().openedWorkspaces.has('workspace-remote-opened')).toBe(true);
  });

  it('re-reads after startup when a hint arrived while the bootstrap snapshot was in flight', async () => {
    let resolveStartup: ((snapshot: ReturnType<typeof snapshot>) => void) | null = null;
    globalStateMocks.initializeWorkspaceStartupState.mockImplementation(
      () => new Promise(resolve => {
        resolveStartup = resolve;
      }),
    );
    const handler = captureCatalogHandler();
    const manager = await getFreshWorkspaceManager();
    const initializePromise = manager.initialize();
    await flushAsyncWork();

    globalStateMocks.getCurrentWorkspace.mockResolvedValue(localWorkspace);
    globalStateMocks.getRecentWorkspaces.mockResolvedValue([localWorkspace, remoteOpenedWorkspace]);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([localWorkspace, remoteOpenedWorkspace]);
    handler.current()?.({ payload: { revision: 9 } });
    expect(globalStateMocks.getOpenedWorkspaces).not.toHaveBeenCalled();

    resolveStartup?.(snapshot([localWorkspace], localWorkspace));
    await initializePromise;
    await flushAsyncWork();

    expect(globalStateMocks.getOpenedWorkspaces).toHaveBeenCalledTimes(1);
    expect(manager.getState().openedWorkspaces.has('workspace-remote-opened')).toBe(true);
  });

  it('ignores hints mirrored from a peer device while the local surface is rendered', async () => {
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue(
      snapshot([localWorkspace], localWorkspace),
    );
    const handler = captureCatalogHandler();
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();

    handler.current()?.({ payload: { revision: 2, __openbitfunSourceDeviceId: 'device-b' } });
    await flushAsyncWork();

    expect(globalStateMocks.getOpenedWorkspaces).not.toHaveBeenCalled();
    expect(manager.getState().openedWorkspaces.size).toBe(1);
  });
});

describe('WorkspaceManager state-quiet loading and error events', () => {
  const workspace = {
    id: 'workspace-quiet',
    name: 'Quiet repo',
    rootPath: '/repo/quiet',
    workspaceKind: 'normal',
    identity: null,
  };

  beforeEach(() => {
    vi.clearAllMocks();
    configureGlobalState();
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0,
      recentWorkspaces: [workspace],
      openedWorkspaces: [workspace],
      currentWorkspace: workspace,
      legacyRemoteWorkspace: null,
    });
    globalStateMocks.getCurrentWorkspace.mockResolvedValue(workspace);
    globalStateMocks.getRecentWorkspaces.mockResolvedValue([workspace]);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([workspace]);
  });

  function collectEvents(manager: {
    addEventListener: (listener: (event: { type: string }) => void) => () => void;
  }): { types: string[]; unsubscribe: () => void } {
    const types: string[] = [];
    const unsubscribe = manager.addEventListener(event => {
      types.push(event.type);
    });
    return { types, unsubscribe };
  }

  it('does not emit an instant-failure loading/error pair when a load starts cleanly', async () => {
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();

    const { types } = collectEvents(manager);
    globalStateMocks.scanWorkspaceInfo.mockResolvedValue(workspace);
    await manager.scanWorkspaceInfo();

    expect(types).not.toContain('workspace:error');
    expect(types.filter(type => type === 'workspace:loading')).toEqual(['workspace:loading']);
  });

  it('still reports real failures through workspace:error', async () => {
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();

    const { types } = collectEvents(manager);
    globalStateMocks.scanWorkspaceInfo.mockRejectedValue(new Error('scan failed'));
    await expect(manager.scanWorkspaceInfo()).rejects.toThrow('scan failed');

    expect(types).toContain('workspace:error');
    expect(manager.getState().error).toBe('scan failed');
  });
});

describe('WorkspaceManager openWorkspace promotion', () => {
  const workspaceA = {
    id: 'workspace-a',
    name: 'Repo A',
    rootPath: '/repo/a',
    workspaceKind: 'normal',
    identity: null,
  };
  const workspaceB = {
    id: 'workspace-b',
    name: 'Repo B',
    rootPath: '/repo/b',
    workspaceKind: 'normal',
    identity: null,
  };
  const workspaceC = {
    id: 'workspace-c',
    name: 'Repo C',
    rootPath: '/repo/c',
    workspaceKind: 'remote',
    identity: null,
  };

  beforeEach(() => {
    vi.clearAllMocks();
    configureGlobalState();
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0,
      recentWorkspaces: [workspaceA, workspaceB, workspaceC],
      openedWorkspaces: [workspaceA, workspaceB, workspaceC],
      currentWorkspace: workspaceA,
      legacyRemoteWorkspace: null,
    });
    globalStateMocks.getCurrentWorkspace.mockResolvedValue(workspaceA);
    globalStateMocks.getRecentWorkspaces.mockResolvedValue([workspaceA, workspaceB, workspaceC]);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([workspaceA, workspaceB, workspaceC]);
    globalStateMocks.reorderOpenedWorkspaces.mockResolvedValue(undefined);
  });

  it('adopts the backend-promoted order for an already-opened local workspace without a second reorder RPC', async () => {
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();

    globalStateMocks.openWorkspace.mockResolvedValue(workspaceB);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([workspaceB, workspaceA, workspaceC]);
    await manager.openWorkspace(workspaceB.rootPath);

    expect(Array.from(manager.getState().openedWorkspaces.keys())).toEqual([
      workspaceB.id,
      workspaceA.id,
      workspaceC.id,
    ]);
    expect(globalStateMocks.reorderOpenedWorkspaces).not.toHaveBeenCalled();
  });

  it('adopts the backend-promoted order for an already-opened remote workspace without a second reorder RPC', async () => {
    const manager = await getFreshWorkspaceManager();
    await manager.initialize();

    globalStateMocks.openRemoteWorkspace.mockResolvedValue(workspaceC);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([workspaceC, workspaceA, workspaceB]);
    await manager.openRemoteWorkspace({
      connectionId: 'conn-1',
      connectionName: 'Remote',
      remotePath: '/repo/c',
    });

    expect(Array.from(manager.getState().openedWorkspaces.keys())).toEqual([
      workspaceC.id,
      workspaceA.id,
      workspaceB.id,
    ]);
    expect(globalStateMocks.reorderOpenedWorkspaces).not.toHaveBeenCalled();
  });

  it('keeps local and peer display orders independent, including identical workspace IDs', async () => {
    const { manager, deviceSurface } = await getFreshWorkspaceHarness();
    await manager.initialize();

    deviceSurface.activateSurface('peer-b');
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValueOnce({
      cleanupRemovedCount: 0,
      recentWorkspaces: [workspaceC, workspaceA, workspaceB],
      openedWorkspaces: [workspaceC, workspaceA, workspaceB],
      currentWorkspace: workspaceC,
      legacyRemoteWorkspace: null,
    });
    await manager.initialize();

    globalStateMocks.openWorkspace.mockResolvedValue(workspaceB);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([workspaceB, workspaceC, workspaceA]);
    await manager.openWorkspace(workspaceB.rootPath);

    expect(Array.from(manager.getState().openedWorkspaces.keys())).toEqual([
      workspaceB.id,
      workspaceC.id,
      workspaceA.id,
    ]);
    expect(manager.getSurfaceOpenedWorkspaces(deviceSurface.LOCAL_SURFACE_ID).map(w => w.id))
      .toEqual([workspaceA.id, workspaceB.id, workspaceC.id]);
    deviceSurface.activateSurface(deviceSurface.LOCAL_SURFACE_ID);
    expect(Array.from(manager.getState().openedWorkspaces.keys()))
      .toEqual([workspaceA.id, workspaceB.id, workspaceC.id]);
    deviceSurface.activateSurface('peer-b');
    expect(Array.from(manager.getState().openedWorkspaces.keys()))
      .toEqual([workspaceB.id, workspaceC.id, workspaceA.id]);
  });

  it('snapshots pre-promotion geometry before state updates and emits reveal afterwards', async () => {
    const { manager, displayOrder } = await getFreshWorkspaceHarness();
    await manager.initialize();

    globalStateMocks.openWorkspace.mockResolvedValue(workspaceB);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([workspaceB, workspaceA, workspaceC]);
    const steps: string[] = [];
    displayOrder.subscribeWorkspacePromotionSnapshot((_surfaceId, workspaceId) => {
      steps.push(`snapshot:${workspaceId}`);
      // At pre-promotion snapshot time, state still reflects the old layout.
      expect(Array.from(manager.getState().openedWorkspaces.keys()))
        .toEqual([workspaceA.id, workspaceB.id, workspaceC.id]);
    });
    displayOrder.subscribeWorkspaceReveal(request => {
      steps.push(`reveal:${request.workspaceId}:${request.promoted}`);
    });
    manager.addEventListener(event => { steps.push(event.type); });

    const opened = await manager.openWorkspace(workspaceB.rootPath);
    expect(opened).toBe(workspaceB);
    expect(Array.from(manager.getState().openedWorkspaces.keys()))
      .toEqual([workspaceB.id, workspaceA.id, workspaceC.id]);
    expect(manager.getState().loading).toBe(false);
    expect(steps).toEqual([
      'workspace:loading',
      `snapshot:${workspaceB.id}`,
      'workspace:opened',
      `reveal:${workspaceB.id}:true`,
    ]);
    expect(globalStateMocks.reorderOpenedWorkspaces).not.toHaveBeenCalled();
  });

  it('does not leak reveal requests onto a new device switched inside a workspace:opened listener', async () => {
    const { manager, deviceSurface, displayOrder } = await getFreshWorkspaceHarness();
    await manager.initialize();
    globalStateMocks.openWorkspace.mockResolvedValue(workspaceB);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([workspaceB, workspaceA, workspaceC]);
    const reveals: string[] = [];
    displayOrder.subscribeWorkspaceReveal(request => { reveals.push(request.surfaceId); });

    manager.addEventListener(event => {
      if (event.type === 'workspace:opened') {
        deviceSurface.activateSurface('peer-b');
      }
    });

    await manager.openWorkspace(workspaceB.rootPath);
    await flushAsyncWork();

    expect(reveals).toEqual([]);
    expect(manager.getSurfaceOpenedWorkspaces(deviceSurface.LOCAL_SURFACE_ID)[0]?.id).toBe(workspaceB.id);
    expect(manager.getState().openedWorkspaces.size).toBe(0);
  });

  it('normalizes transport failures to SurfaceChangedError when the device changed mid-open', async () => {
    const { manager, deviceSurface } = await getFreshWorkspaceHarness();
    await manager.initialize();
    globalStateMocks.openWorkspace.mockImplementation(async () => {
      deviceSurface.activateSurface('peer-b');
      throw new Error('peer link closed');
    });

    const error = await manager.openWorkspace(workspaceB.rootPath).catch(cause => cause);
    expect(isSurfaceChangedError(error)).toBe(true);
    expect(manager.getState().error).toBeNull();
  });

  it('does not snapshot promotion or emit reveal when opening an assistant directory', async () => {
    const assistantA = { ...workspaceA, id: 'assistant-a', rootPath: '/assistant/a', workspaceKind: 'assistant' };
    const assistantB = { ...workspaceA, id: 'assistant-b', rootPath: '/assistant/b', workspaceKind: 'assistant' };
    const opened = [assistantA, assistantB, workspaceA];
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0,
      recentWorkspaces: opened,
      openedWorkspaces: opened,
      currentWorkspace: workspaceA,
      legacyRemoteWorkspace: null,
    });
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue(opened);

    const { manager, displayOrder } = await getFreshWorkspaceHarness();
    await manager.initialize();
    const reveals: string[] = [];
    displayOrder.subscribeWorkspaceReveal(request => { reveals.push(request.workspaceId); });

    globalStateMocks.openWorkspace.mockResolvedValue(assistantB);
    await manager.openWorkspace(assistantB.rootPath);

    expect(Array.from(manager.getState().openedWorkspaces.keys()))
      .toEqual([assistantA.id, assistantB.id, workspaceA.id]);
    expect(globalStateMocks.reorderOpenedWorkspaces).not.toHaveBeenCalled();
    expect(reveals).toEqual([]);
  });

  it('detects promotion within the project section while leaving the assistant slot alone', async () => {
    const assistant = { ...workspaceA, id: 'assistant', rootPath: '/assistant', workspaceKind: 'assistant' };
    const opened = [assistant, workspaceA, workspaceB];
    globalStateMocks.initializeWorkspaceStartupState.mockResolvedValue({
      cleanupRemovedCount: 0,
      recentWorkspaces: opened,
      openedWorkspaces: opened,
      currentWorkspace: workspaceA,
      legacyRemoteWorkspace: null,
    });
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue(opened);
    const { manager, displayOrder } = await getFreshWorkspaceHarness();
    await manager.initialize();
    const reveals: Array<{ id: string; promoted: boolean }> = [];
    displayOrder.subscribeWorkspaceReveal(req => {
      reveals.push({ id: req.workspaceId, promoted: req.promoted });
    });

    globalStateMocks.openWorkspace.mockResolvedValue(workspaceA);
    await manager.openWorkspace(workspaceA.rootPath);

    globalStateMocks.openWorkspace.mockResolvedValue(workspaceB);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([assistant, workspaceB, workspaceA]);
    await manager.openWorkspace(workspaceB.rootPath);
    expect(Array.from(manager.getState().openedWorkspaces.keys()))
      .toEqual([assistant.id, workspaceB.id, workspaceA.id]);
    expect(reveals).toEqual([
      { id: workspaceA.id, promoted: false },
      { id: workspaceB.id, promoted: true },
    ]);
    expect(globalStateMocks.reorderOpenedWorkspaces).not.toHaveBeenCalled();
  });

  it('forwards promoteIfOpened: false on background remote restore and skips reveal', async () => {
    const { manager, displayOrder } = await getFreshWorkspaceHarness();
    await manager.initialize();
    const reveals: string[] = [];
    displayOrder.subscribeWorkspaceReveal(req => { reveals.push(req.workspaceId); });
    globalStateMocks.openRemoteWorkspace.mockResolvedValue(workspaceC);

    await manager.openRemoteWorkspace({
      connectionId: 'conn-1', connectionName: 'Remote', remotePath: workspaceC.rootPath,
    }, { promoteIfOpened: false });

    expect(Array.from(manager.getState().openedWorkspaces.keys()))
      .toEqual([workspaceA.id, workspaceB.id, workspaceC.id]);
    expect(reveals).toEqual([]);
    expect(globalStateMocks.openRemoteWorkspace.mock.calls[0][4]).toEqual({ promoteIfOpened: false });
  });

  it('reveals without promotion motion for a newly opened workspace', async () => {
    const { manager, displayOrder } = await getFreshWorkspaceHarness();
    await manager.initialize();
    const added = { ...workspaceA, id: 'workspace-new', rootPath: '/repo/new' };
    const reveals: Array<{ id: string; promoted: boolean }> = [];
    displayOrder.subscribeWorkspaceReveal(req => {
      reveals.push({ id: req.workspaceId, promoted: req.promoted });
    });
    globalStateMocks.openWorkspace.mockResolvedValue(added);
    globalStateMocks.getOpenedWorkspaces.mockResolvedValue([added, workspaceA, workspaceB, workspaceC]);

    await manager.openWorkspace(added.rootPath);

    expect(Array.from(manager.getState().openedWorkspaces.keys()))
      .toEqual([added.id, workspaceA.id, workspaceB.id, workspaceC.id]);
    expect(reveals).toEqual([{ id: added.id, promoted: false }]);
    expect(globalStateMocks.reorderOpenedWorkspaces).not.toHaveBeenCalled();
  });
});
