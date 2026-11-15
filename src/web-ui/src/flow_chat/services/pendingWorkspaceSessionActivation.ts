/**
 * Tracks an explicit cross-workspace session activation in flight so workspace
 * bootstrap (`FlowChatManager.initializeWorkspace`) loads metadata without
 * racing to auto-select and hydrate a different latest session.
 */

interface PendingWorkspaceSessionActivation {
  surfaceId: string;
  workspaceId: string;
  sessionId: string;
  request: number;
}

let currentPendingActivation: PendingWorkspaceSessionActivation | null = null;

export function setPendingWorkspaceSessionActivation(
  next: PendingWorkspaceSessionActivation,
): void {
  currentPendingActivation = next;
}

export function clearPendingWorkspaceSessionActivation(request: number): void {
  if (currentPendingActivation?.request === request) {
    currentPendingActivation = null;
  }
}

export function getPendingWorkspaceSessionActivation(
  surfaceId: string,
  workspaceId: string,
): string | null {
  if (
    currentPendingActivation
    && currentPendingActivation.surfaceId === surfaceId
    && currentPendingActivation.workspaceId === workspaceId
  ) {
    return currentPendingActivation.sessionId;
  }
  return null;
}
