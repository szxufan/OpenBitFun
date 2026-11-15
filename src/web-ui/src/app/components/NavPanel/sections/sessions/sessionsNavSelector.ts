import type { FlowChatState, Session } from '../../../../../flow_chat/types/flow-chat';
import { sessionOwningWorkspaceId } from '../../../../../flow_chat/utils/sessionOrdering';

type DispatchTargetKind = 'local' | 'ssh' | 'device';

export interface SessionsNavSelectorOptions {
  includeSession?: (session: Session) => boolean;
}

interface SessionNavSignature {
  sessionRef: Session;
  isSelected: boolean;
  sessionId: string;
  owningWorkspaceId: string | undefined;
  persistedStatus: Session['persistedStatus'];
  importedSource: string | undefined;
  isTransient: boolean | undefined;
  sessionKind: Session['sessionKind'];
  parentSessionId: string | undefined;
  parentToolCallId: string | undefined;
  subagentType: string | undefined;
  workspacePath: string | undefined;
  mode: string | undefined;
  needsUserAttention: Session['needsUserAttention'];
  hasUnreadCompletion: Session['hasUnreadCompletion'];
  latestTurnStatus: string | undefined;
  title: string | undefined;
  dispatchTargetKind: DispatchTargetKind;
  dispatchConnectionId: string | undefined;
  dispatchDeviceId: string | undefined;
  dispatchWorkspacePath: string | undefined;
  dispatchDisplayName: string | undefined;
  dispatchJobState: Session['config']['dispatchJobState'];
}

function readSessionNavSignature(session: Session, isSelected: boolean): SessionNavSignature {
  const latestTurn = session.dialogTurns[session.dialogTurns.length - 1];
  const dispatchTarget = session.config.dispatchTarget;

  return {
    sessionRef: session,
    isSelected,
    sessionId: session.sessionId,
    owningWorkspaceId: sessionOwningWorkspaceId(session),
    persistedStatus: session.persistedStatus,
    importedSource: session.importedSource,
    isTransient: session.isTransient,
    sessionKind: session.sessionKind,
    parentSessionId: session.parentSessionId,
    parentToolCallId: session.parentToolCallId,
    subagentType: session.subagentType,
    workspacePath: session.workspacePath,
    mode: session.mode,
    needsUserAttention: session.needsUserAttention,
    hasUnreadCompletion: session.hasUnreadCompletion,
    latestTurnStatus: latestTurn?.status,
    title: session.title,
    dispatchTargetKind: dispatchTarget?.kind ?? 'local',
    dispatchConnectionId: dispatchTarget?.kind === 'ssh' ? dispatchTarget.connectionId : undefined,
    dispatchDeviceId: dispatchTarget?.kind === 'device' ? dispatchTarget.deviceId : undefined,
    dispatchWorkspacePath: dispatchTarget?.kind === 'local' ? undefined : dispatchTarget?.workspacePath,
    dispatchDisplayName: dispatchTarget?.kind === 'local' ? undefined : dispatchTarget?.displayName,
    dispatchJobState: session.config.dispatchJobState,
  };
}

function sameSessionNavSignature(previous: SessionNavSignature, session: Session): boolean {
  const dispatchTarget = session.config.dispatchTarget;
  const dispatchTargetKind = dispatchTarget?.kind ?? 'local';
  const latestTurn = session.dialogTurns[session.dialogTurns.length - 1];

  return previous.sessionId === session.sessionId
    && previous.owningWorkspaceId === sessionOwningWorkspaceId(session)
    && previous.persistedStatus === session.persistedStatus
    && previous.importedSource === session.importedSource
    && previous.isTransient === session.isTransient
    && previous.sessionKind === session.sessionKind
    && previous.parentSessionId === session.parentSessionId
    && previous.parentToolCallId === session.parentToolCallId
    && previous.subagentType === session.subagentType
    && previous.workspacePath === session.workspacePath
    && previous.mode === session.mode
    && previous.needsUserAttention === session.needsUserAttention
    && previous.hasUnreadCompletion === session.hasUnreadCompletion
    && previous.latestTurnStatus === latestTurn?.status
    && previous.title === session.title
    && previous.dispatchTargetKind === dispatchTargetKind
    && previous.dispatchConnectionId === (dispatchTarget?.kind === 'ssh' ? dispatchTarget.connectionId : undefined)
    && previous.dispatchDeviceId === (dispatchTarget?.kind === 'device' ? dispatchTarget.deviceId : undefined)
    && previous.dispatchWorkspacePath === (dispatchTarget?.kind === 'local' ? undefined : dispatchTarget?.workspacePath)
    && previous.dispatchDisplayName === (dispatchTarget?.kind === 'local' ? undefined : dispatchTarget?.displayName)
    && previous.dispatchJobState === session.config.dispatchJobState;
}

/**
 * Selects only the fields rendered by the sessions navigation.
 *
 * The selector is called for every FlowChatStore notification. It keeps a
 * primitive revision as its result, so stream updates do not rebuild a large
 * joined string merely to discover that the navigation did not change.
 */
export function createSessionsNavSelector(
  options?: SessionsNavSelectorOptions,
): (state: FlowChatState) => number {
  const includeSession = options?.includeSession;
  let orderedSessionIds: string[] = [];
  const signatures = new Map<string, SessionNavSignature>();
  let revision = 0;

  return (state: FlowChatState): number => {
    let changed = false;
    let orderChanged = false;
    let index = 0;
    let nextOrderedIds: string[] | null = null;

    for (const session of state.sessions.values()) {
      if (includeSession && !includeSession(session)) {
        continue;
      }

      if (orderedSessionIds[index] !== session.sessionId) {
        changed = true;
        orderChanged = true;
      }
      if (orderChanged) {
        if (!nextOrderedIds) {
          nextOrderedIds = orderedSessionIds.slice(0, index);
        }
        nextOrderedIds.push(session.sessionId);
      }

      const isSelected = state.activeSessionId === session.sessionId;
      const previous = signatures.get(session.sessionId);
      if (previous && previous.sessionRef === session && previous.isSelected === isSelected) {
        index += 1;
        continue;
      }

      if (!previous || previous.isSelected !== isSelected || !sameSessionNavSignature(previous, session)) {
        signatures.set(session.sessionId, readSessionNavSignature(session, isSelected));
        changed = true;
      } else {
        previous.sessionRef = session;
      }
      index += 1;
    }

    if (index !== orderedSessionIds.length) {
      changed = true;
      orderChanged = true;
      if (!nextOrderedIds) {
        nextOrderedIds = orderedSessionIds.slice(0, index);
      }
    }

    if (changed || revision === 0) {
      // Initial read on an empty session list still establishes revision 0 -> 0 unless non-empty;
      // preserve exact behavior when empty vs populated.
      if (changed) {
        revision += 1;
      }
      if (orderChanged && nextOrderedIds) {
        orderedSessionIds = nextOrderedIds;
        if (signatures.size !== orderedSessionIds.length) {
          const retainedIds = new Set(orderedSessionIds);
          for (const sessionId of signatures.keys()) {
            if (!retainedIds.has(sessionId)) {
              signatures.delete(sessionId);
            }
          }
        }
      }
    }

    return revision;
  };
}
