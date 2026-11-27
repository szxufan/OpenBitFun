import { getActiveSurfaceId } from '@/infrastructure/peer-device/deviceSurface';
import { SessionExecutionState, stateMachineManager } from '../state-machine';
import { isComposerSubmissionPending, sessionComposerStore } from '../store/sessionComposerStore';
import type { FlowChatState, Session, SessionConfig } from '../types/flow-chat';
import { isAcpFlowSession } from '../utils/acpSession';
import { isSessionBindingLocked } from '../utils/sessionLifecycle';
import { sessionOwningWorkspaceId } from '../utils/sessionOrdering';
import { pendingQueueManager } from './flow-chat-manager/PendingQueueModule';

// Paths are execution projections; identity is the captured device's workspace ID.
const WORKSPACE_CONFIG_KEYS = new Set([
  'workspaceId', 'workspacePath', 'projectWorkspaceId', 'projectWorkspacePath',
  'remoteConnectionId', 'remoteSshHost', 'agentType',
]);

/** Only controller-owned, unsubmitted drafts are proof of an empty conversation. */
export function findReusableSessionDraft(
  state: FlowChatState,
  workspaceId: string,
  mode: string,
  config: SessionConfig,
): string | null {
  if (config.executionTargetRequest || config.dispatchTargetRequest
    || config.dispatchTarget || config.dispatchJobId) return null;

  const surfaceId = getActiveSurfaceId();
  const reusable = (session: Session): boolean => {
    if (session.draft?.phase !== 'editing' || session.draft.workspaceId !== workspaceId
      || sessionOwningWorkspaceId(session) !== workspaceId || session.mode !== mode
      || isSessionBindingLocked(session) || session.dialogTurns.length > 0
      || session.persistedTurnCount || session.isPartial
      || session.sessionKind !== 'normal' || session.parentSessionId || session.isTransient
      || session.agentBackedTransient || session.btwThreads?.length || session.externalHistory
      || isAcpFlowSession(session) || session.error || session.persistedStatus === 'archived'
      || session.config.executionTarget || session.config.executionTargetRequest
      || session.config.dispatchTargetRequest || session.config.dispatchTarget || session.config.dispatchJobId) {
      return false;
    }
    // Explicit caller choices must never be lost by selecting a different draft.
    if (Object.entries(config).some(([key, value]) => value !== undefined
      && !WORKSPACE_CONFIG_KEYS.has(key)
      && JSON.stringify(session.config[key as keyof SessionConfig]) !== JSON.stringify(value))) return false;
    const composer = sessionComposerStore.getState().getDraft(session.sessionId, surfaceId);
    if ((composer.value.trim() && composer.prefillSource !== 'assistant-bootstrap')
      || composer.contexts.length || Object.keys(composer.pendingLargePastes).length
      || isComposerSubmissionPending(session.sessionId, surfaceId)
      || pendingQueueManager.list(session.sessionId).length
      || stateMachineManager.getCurrentState(session.sessionId) !== SessionExecutionState.IDLE) return false;
    return true;
  };

  const active = state.activeSessionId ? state.sessions.get(state.activeSessionId) : undefined;
  if (active && reusable(active)) return active.sessionId;
  let latest: Session | undefined;
  for (const session of state.sessions.values()) {
    if (reusable(session) && (!latest || session.lastActiveAt > latest.lastActiveAt)) latest = session;
  }
  return latest?.sessionId ?? null;
}
