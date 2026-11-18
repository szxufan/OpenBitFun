import { workspaceManager } from '@/infrastructure/services/business/workspaceManager';
import { configManager } from '@/infrastructure/config/services/ConfigManager';
import type { TerminalConfig } from '@/infrastructure/config/types';
import { getTerminalService } from '@/tools/terminal/services/TerminalService';
import type { CreateSessionRequest, HostTerminalWorkspaceTarget, SessionResponse } from '@/tools/terminal/types/session';
import { getActiveSurfaceScope } from '@/infrastructure/peer-device/deviceSurface';

export interface CreateManualTerminalSessionOptions {
  workspaceId: string;
  workspacePath?: string;
  hostWorkspaceTarget?: HostTerminalWorkspaceTarget;
  shellType?: string;
  shellId?: string;
  name?: string;
  sessionId?: string;
}

async function getDefaultShellPreference(): Promise<string | undefined> {
  try {
    const config = await configManager.getConfig<TerminalConfig>('terminal');
    return config?.default_shell || undefined;
  } catch {
    return undefined;
  }
}

async function resolveDefaultShellSelection(
  service: ReturnType<typeof getTerminalService>,
): Promise<Pick<CreateSessionRequest, 'shellId' | 'shellType'>> {
  const preference = await getDefaultShellPreference();
  if (!preference) {
    return {};
  }

  try {
    const shell = (await service.getAvailableShells()).find(
      (candidate) => candidate.path === preference,
    );
    return shell ? { shellId: shell.id, shellType: shell.shellType } : {};
  } catch {
    return {};
  }
}

/** Create a regular user terminal without requiring the Shell navigation UI. */
export async function createManualTerminalSession(
  options: CreateManualTerminalSessionOptions,
): Promise<SessionResponse> {
  const { workspaceId, hostWorkspaceTarget } = options;
  const requireWorkspace = () => {
    const state = workspaceManager.getState();
    const workspace = state.openedWorkspaces.get(workspaceId)
      ?? state.recentWorkspaces.find(record => record.id === workspaceId);
    if (!workspace) throw new Error('Terminal workspace ID is unavailable');
    if (hostWorkspaceTarget && (workspace.rootPath !== hostWorkspaceTarget.rootPath
      || (workspace.workspaceKind === 'remote'
        ? !workspace.connectionId || workspace.connectionId !== hostWorkspaceTarget.connectionId
        : hostWorkspaceTarget.connectionId !== undefined))) {
      throw new Error('Terminal owner workspace changed');
    }
    return workspace;
  };
  const workspace = requireWorkspace();
  const service = getTerminalService();
  const scope = getActiveSurfaceScope();
  await service.connect();
  scope.assertCurrent('create manual terminal');

  const usesSsh = !hostWorkspaceTarget && workspace.workspaceKind === 'remote';
  const [sessions, shellSelection] = await Promise.all([
    service.listSessions(),
    usesSsh ? Promise.resolve({}) : resolveDefaultShellSelection(service),
  ]);
  scope.assertCurrent('create manual terminal');
  if (hostWorkspaceTarget) requireWorkspace();
  const manualCount = sessions.filter((session) => session.source === 'manual').length;

  return service.createSession({
    workspaceId,
    ...(hostWorkspaceTarget && { hostWorkspaceTarget }),
    workingDirectory: hostWorkspaceTarget?.connectionId ? undefined : options.workspacePath,
    name: options.name ?? `Shell ${manualCount + 1}`,
    sessionId: options.sessionId,
    ...(options.shellId ? { shellId: options.shellId, shellType: options.shellType }
      : options.shellType && options.shellType !== 'Remote' ? { shellType: options.shellType } : shellSelection),
    source: 'manual',
  });
}
