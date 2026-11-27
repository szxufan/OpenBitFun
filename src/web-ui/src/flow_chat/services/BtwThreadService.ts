import { configManager } from '@/infrastructure/config/services/ConfigManager';
import { btwModelSelection, supportsBtwModelDefaults } from '@/infrastructure/config/services/btwModelDefaults';
import { modelPoolAPI } from '@/infrastructure/api/service-api/ModelPoolAPI';
import { agentAPI, btwAPI } from '@/infrastructure/api';
import type { BtwInitialModelTarget, BtwAskStreamResponse } from '@/infrastructure/api/service-api/BtwAPI';
import { i18nService } from '@/infrastructure/i18n';
import { notificationService } from '@/shared/notification-system';
import { flowChatStore } from '../store/FlowChatStore';
import { stateMachineManager } from '../state-machine';
import { flowChatManager } from './FlowChatManager';
import type { Session } from '../types/flow-chat';
import type { SessionKind, SessionRelationship } from '@/shared/types/session-history';
import type {
  ReviewTargetEvidence,
  ReviewTeamRunManifest,
} from '@/shared/services/reviewTeamService';
import type { ImagePayload } from '../utils/imagePayload';
import { absoluteSessionTurnIndexForId } from '../utils/flowChatTurnOrdinal';
import { requireSessionWorkspaceId, sessionWorkspaceId } from '../utils/sessionWorkspace';
import { getActiveSurfaceScope } from '@/infrastructure/peer-device/deviceSurface';
import { classifyPeerHostRefusal } from '@/infrastructure/peer-device/peerHostRefusal';
import { isPeerReplyLost } from '@/infrastructure/peer-device/peerReplyLost';
import { assertSessionConversationCanSubmit } from '../session-drivers/conversationCapability';
import { trackBtwSessionSubmission } from './btwSessionSubmission';
import { createLogger } from '@/shared/utils/logger';

const log = createLogger('BtwThreadService');

export function createBtwRequestId(prefix = 'btw'): string {
  try {
    const fn = (globalThis as any)?.crypto?.randomUUID as (() => string) | undefined;
    if (fn) return fn();
  } catch {
    // ignore
  }
  return `${prefix}_${Date.now()}_${Math.random().toString(16).slice(2)}`;
}

function toOneLine(input: string): string {
  return input.replace(/\s+/g, ' ').trim();
}

function buildPersistentReviewSessionId(requestId: string): string {
  const safeRequestId = requestId.trim().replace(/[^A-Za-z0-9_-]/g, '_');
  return `review_child_${safeRequestId}`;
}

function buildChildSessionName(question: string): string {
  const one = toOneLine(question);
  const clipped = one.length > 48 ? `${one.slice(0, 48)}…` : one;
  return clipped || 'Side thread';
}

function getParentInterruptionContext(parentSessionId: string): { parentDialogTurnId?: string; parentTurnIndex?: number } {
  const machine = stateMachineManager.get(parentSessionId);
  const ctx = machine?.getContext?.();
  const machineTurnId = ctx?.currentDialogTurnId || undefined;

  const session = flowChatStore.getState().sessions.get(parentSessionId);
  if (!session) {
    return { parentDialogTurnId: machineTurnId, parentTurnIndex: undefined };
  }

  const parentDialogTurnId = machineTurnId || session.dialogTurns[session.dialogTurns.length - 1]?.id;
  if (!parentDialogTurnId) return { parentDialogTurnId: undefined, parentTurnIndex: undefined };

  return {
    parentDialogTurnId,
    parentTurnIndex: absoluteSessionTurnIndexForId(session, parentDialogTurnId),
  };
}

function requireSession(sessionId: string): Session {
  const session = flowChatStore.getState().sessions.get(sessionId);
  if (!session) {
    throw new Error(`Session not found: ${sessionId}`);
  }
  return session;
}

export async function createBtwChildSession(params: {
  parentSessionId: string;
  /** Owning workspace ID; defaults to the parent session's workspace. */
  workspaceId?: string;
  /** Execution root (IO operand); defaults to the parent session's root. */
  workspacePath?: string;
  childSessionName: string;
  agentType?: string;
  modelName?: string;
  enableTools?: boolean;
  safeMode?: boolean;
  autoCompact?: boolean;
  enableContextCompression?: boolean;
  requestId?: string;
  addMarker?: boolean;
  isTransient?: boolean;
  sessionKind?: Extract<SessionKind, 'btw' | 'review' | 'deep_review'>;
  deepReviewRunManifest?: ReviewTeamRunManifest;
  reviewTargetEvidence?: ReviewTargetEvidence;
  reviewTargetFilePaths?: string[];
}): Promise<{
  requestId: string;
  childSessionId: string;
  parentDialogTurnId?: string;
  parentTurnIndex?: number;
}> {
  const { parentSessionId } = params;
  const requestId = params.requestId || createBtwRequestId('btw');
  const childSessionKind = params.sessionKind ?? 'btw';
  const shouldPersistStandaloneSession =
    !params.isTransient && childSessionKind !== 'btw';
  const createdAt = Date.now();
  const { parentDialogTurnId, parentTurnIndex } = getParentInterruptionContext(parentSessionId);

  const parentSession = flowChatStore.getState().sessions.get(parentSessionId);
  // The child inherits the parent's workspace identity; the path is only the
  // execution root the backend runs the child in.
  const workspaceId = params.workspaceId || sessionWorkspaceId(parentSession);
  if (!workspaceId) {
    throw new Error(`Workspace ID is required for BTW child session: ${parentSessionId}`);
  }
  const workspacePath = params.workspacePath || parentSession?.workspacePath;
  if (!workspacePath) {
    throw new Error(`Workspace path is required for BTW child session: ${parentSessionId}`);
  }

  const agentType = params.agentType || parentSession?.mode || 'Standard';
  const modelName = params.modelName || parentSession?.config?.modelName || 'default';
  const childSessionName = params.childSessionName.trim() || 'Side thread';
  const remoteConnectionId = parentSession?.remoteConnectionId;
  const remoteSshHost = parentSession?.remoteSshHost;
  const projectWorkspacePath =
    parentSession?.projectWorkspacePath
    || parentSession?.config.projectWorkspacePath
    || workspacePath;
  const inheritedExecutionTarget = parentSession?.config.executionTarget;
  const relationship: SessionRelationship | undefined =
    childSessionKind === 'btw'
      ? undefined
      : {
          kind: childSessionKind,
          parentSessionId,
          parentRequestId: requestId,
          parentDialogTurnId,
          parentTurnIndex,
        };
  const createdSession = shouldPersistStandaloneSession
    ? await agentAPI.createSession({
          sessionId: buildPersistentReviewSessionId(requestId),
          sessionName: childSessionName,
          agentType,
          workspacePath,
          projectWorkspacePath,
          executionTarget: inheritedExecutionTarget?.worktreeId
            ? {
                kind: 'existingWorktree',
                worktreeId: inheritedExecutionTarget.worktreeId,
              }
            : { kind: 'local' },
          workspaceId,
          remoteConnectionId,
          remoteSshHost,
          relationship,
          deepReviewRunManifest: params.deepReviewRunManifest,
          reviewTargetEvidence: params.reviewTargetEvidence,
          config: {
            modelName,
            enableTools: params.enableTools ?? false,
            safeMode: params.safeMode ?? true,
            autoCompact: params.autoCompact ?? true,
            enableContextCompression: params.enableContextCompression ?? true,
            remoteConnectionId,
            remoteSshHost,
          },
        })
    : null;
  const childSessionId = createdSession?.sessionId ?? createBtwRequestId('btw_session');
  const childWorkspacePath = createdSession?.workspacePath || workspacePath;
  flowChatStore.addExternalSession(
    childSessionId,
    childSessionName,
    agentType,
    childWorkspacePath,
    {
      parentSessionId,
      sessionKind: childSessionKind,
      btwOrigin: {
        requestId,
        parentSessionId,
        parentDialogTurnId,
        parentTurnIndex,
      },
      deepReviewRunManifest: params.deepReviewRunManifest,
      reviewTargetEvidence: params.reviewTargetEvidence,
      reviewTargetFilePaths: params.reviewTargetFilePaths,
      projectWorkspacePath:
        createdSession?.projectWorkspacePath || projectWorkspacePath,
      // The child owns the parent's project, so navigation and persistence
      // resolve both to the same group.
      projectWorkspaceId:
        parentSession?.projectWorkspaceId
        || parentSession?.config.projectWorkspaceId,
      executionTarget:
        createdSession?.executionTarget || inheritedExecutionTarget,
      workspaceId: createdSession?.workspaceId || workspaceId,
      isTransient: childSessionKind === 'btw' || (params.isTransient ?? false),
      agentBackedTransient: childSessionKind === 'btw' || (params.isTransient ?? false),
    },
    remoteConnectionId,
    remoteSshHost
  );
  flowChatStore.updateSessionRelationship(childSessionId, {
    parentSessionId,
    sessionKind: childSessionKind,
  });
  flowChatStore.updateSessionBtwOrigin(childSessionId, {
    requestId,
    parentSessionId,
    parentDialogTurnId,
    parentTurnIndex,
  }, childSessionKind);

  if (params.addMarker ?? false) {
    flowChatStore.addBtwThreadMarker(parentSessionId, {
      requestId,
      childSessionId,
      title: childSessionName,
      status: 'running',
      createdAt,
      parentDialogTurnId,
      parentTurnIndex,
    });
  }

  return {
    requestId,
    childSessionId,
    parentDialogTurnId,
    parentTurnIndex,
  };
}

/** Preview only: the host resolves its authoritative default at first submit. */
async function previewBtwSessionModel(childSessionId: string): Promise<void> {
  if (!supportsBtwModelDefaults()) return;
  const scope = getActiveSurfaceScope();
  try {
    const raw = await configManager.getConfig<unknown>('ai.btw');
    if (!scope.isCurrent()) return;
    const session = flowChatStore.getState().sessions.get(childSessionId);
    if (!session || session.dialogTurns.length > 0 || session.btwOrigin?.requestId
      || session.config.btwModelSelectionExplicit) return;
    const selection = btwModelSelection(raw);
    if (!selection) throw new Error('Unsupported ai.btw.model_selection');
    if (selection.kind === 'inherit') return;
    flowChatStore.updateSessionModelPool(childSessionId, selection.kind === 'pool' ? selection.pool_id : undefined);
    flowChatStore.updateSessionModelName(childSessionId, selection.kind === 'model' ? selection.model_ref : undefined);
    flowChatStore.updateSessionReasoningPreset(childSessionId, undefined);
  } catch (error) {
    if (scope.isCurrent()) log.warn('Could not preview BTW model defaults; host submission will validate them', { error });
  }
}

export function createBtwSessionPlaceholder(params: {
  parentSessionId: string;
  workspacePath?: string;
  childSessionName: string;
  parentDialogTurnId?: string;
}): { childSessionId: string; parentDialogTurnId?: string; parentTurnIndex?: number } {
  const parentSession = requireSession(params.parentSessionId);
  const workspaceId = requireSessionWorkspaceId(parentSession);
  const workspacePath = params.workspacePath || parentSession.workspacePath;
  if (!workspacePath) {
    throw new Error(`Workspace path is required for BTW child session: ${params.parentSessionId}`);
  }

  const childSessionId = createBtwRequestId('btw_session');
  const childSessionName = params.childSessionName.trim() || 'Side thread';
  const { parentDialogTurnId, parentTurnIndex } = params.parentDialogTurnId
    ? { parentDialogTurnId: params.parentDialogTurnId,
        parentTurnIndex: absoluteSessionTurnIndexForId(parentSession, params.parentDialogTurnId) }
    : getParentInterruptionContext(params.parentSessionId);

  flowChatStore.addExternalSession(
    childSessionId,
    childSessionName,
    parentSession.mode || 'Standard',
    workspacePath,
    {
      parentSessionId: params.parentSessionId,
      sessionKind: 'btw',
      btwOrigin: {
        parentSessionId: params.parentSessionId,
        parentDialogTurnId,
        parentTurnIndex,
      },
      isTransient: true,
      agentBackedTransient: true,
      projectWorkspacePath:
        parentSession.projectWorkspacePath
        || parentSession.config.projectWorkspacePath
        || workspacePath,
      projectWorkspaceId:
        parentSession.projectWorkspaceId || parentSession.config.projectWorkspaceId,
      executionTarget: parentSession.config.executionTarget,
      workspaceId,
    },
    parentSession.remoteConnectionId,
    parentSession.remoteSshHost
  );

  flowChatStore.updateSessionRelationship(childSessionId, {
    parentSessionId: params.parentSessionId,
    sessionKind: 'btw',
  });
  if (parentSession.config.modelName) {
    flowChatStore.updateSessionModelName(childSessionId, parentSession.config.modelName);
  }
  flowChatStore.updateSessionReasoningPreset(childSessionId, parentSession.config.reasoningPreset);
  if (parentSession.config.modelPoolId) {
    // Mirror inherited intent for the selector, but let the host snapshot its
    // current binding at first submission. This is not an explicit pool change.
    flowChatStore.updateSessionModelPool(childSessionId, parentSession.config.modelPoolId,
      parentSession.config.modelPoolBinding);
  }

  void previewBtwSessionModel(childSessionId);
  return { childSessionId, parentDialogTurnId, parentTurnIndex };
}

/**
 * A peer host only streams a session's events to a controller that subscribed
 * to it, and the surface subscribes only its active session. A side question
 * renders beside that session, so without its own subscription the host runs
 * the turn while the panel stays empty. The host has created the child once
 * `btw_ask_stream` is accepted; the subscription replays from its records, so
 * events emitted before it opens are not lost. It closes with the session.
 */
function followBtwSessionOnPeer(
  scope: ReturnType<typeof getActiveSurfaceScope>,
  childSessionId: string,
): void {
  if (scope.surfaceId === 'local' || !scope.isCurrent()) return;
  void flowChatStore.loadRelaySessionHistory(childSessionId).catch(error => {
    if (!scope.isCurrent()) return;
    log.warn('Side question stream subscription failed', { childSessionId, error });
  });
}

/**
 * A Relay reconnect can lose the reply after the host already admitted the
 * question, and `btw_ask_stream` is not replayed. Confirm by reading the
 * host's records for the Turn its request id names instead of resubmitting,
 * as a dialog turn with an unknown outcome is confirmed.
 */
async function btwQuestionReachedHost(
  scope: ReturnType<typeof getActiveSurfaceScope>,
  childSessionId: string,
  requestId: string,
  error: unknown,
): Promise<boolean> {
  if (scope.surfaceId === 'local' || !isPeerReplyLost(error) || !scope.isCurrent()) return false;
  try {
    await flowChatStore.loadRelaySessionHistory(childSessionId);
  } catch {
    // The host never created the child, so it never admitted the question.
    return false;
  }
  return scope.isCurrent()
    && flowChatStore.relayHostHasTurn(childSessionId, `btw-turn-${requestId.trim()}`);
}

export async function sendMessageToBtwSession(params: {
  parentSessionId: string;
  childSessionId: string;
  question: string;
  childSessionName?: string;
  modelId?: string;
  imagePayload?: ImagePayload;
  parentDialogTurnId?: string;
  parentTurnIndex?: number;
  userMessageMetadata?: Record<string, unknown>;
  initialModelSelection?: { modelId: string; reasoningPreset?: string };
  initialModelTarget?: BtwInitialModelTarget;
  requestId?: string;
}): Promise<{ requestId: string }> {
  const question = params.question.trim();
  if (!question) {
    notificationService.warning('Please provide a question after /btw');
    throw new Error('Empty /btw question');
  }

  const childSession = requireSession(params.childSessionId);
  if (childSession.sessionKind !== 'btw') {
    throw new Error(`Session is not a /btw session: ${params.childSessionId}`);
  }
  assertSessionConversationCanSubmit(params.childSessionId, childSession,
    i18nService.t('flow-chat:chatInput.targetUnavailable'));

  const scope = getActiveSurfaceScope();
  const requestId = params.requestId ?? createBtwRequestId('btw');
  flowChatStore.updateSessionBtwOrigin(params.childSessionId, {
    ...(childSession.btwOrigin || {}),
    requestId,
    parentSessionId: params.parentSessionId,
    parentDialogTurnId: params.parentDialogTurnId ?? childSession.btwOrigin?.parentDialogTurnId,
    parentTurnIndex: params.parentTurnIndex ?? childSession.btwOrigin?.parentTurnIndex,
  }, 'btw');
  const modelId = params.modelId?.trim();
  const submittedConfig = { ...childSession.config };
  const response: BtwAskStreamResponse = await trackBtwSessionSubmission(params.childSessionId, () => btwAPI.askStream({
    requestId,
    sessionId: params.parentSessionId,
    childSessionId: params.childSessionId,
    childSessionName: params.childSessionName || childSession.title || 'Side thread',
    question,
    ...(modelId ? { modelId } : {}),
    parentDialogTurnId: params.parentDialogTurnId ?? childSession.btwOrigin?.parentDialogTurnId,
    parentTurnIndex: params.parentTurnIndex ?? childSession.btwOrigin?.parentTurnIndex,
    imageContexts: params.imagePayload?.imageContexts,
    ...(params.userMessageMetadata ? { userMessageMetadata: params.userMessageMetadata } : {}),
    ...(params.initialModelSelection ? { initialModelSelection: params.initialModelSelection } : {}),
    ...(params.initialModelTarget ? { initialModelTarget: params.initialModelTarget } : {}),
  }).catch(async error => {
    // Closing waits for this confirmation as part of the submission.
    if (!await btwQuestionReachedHost(scope, params.childSessionId, requestId, error)) {
      if (scope.isCurrent() && supportsBtwModelDefaults() && !classifyPeerHostRefusal(error)) {
        notificationService.error(i18nService.t('flow-chat:btw.submitFailed', {
          error: error instanceof Error ? error.message : String(error),
        }));
      }
      throw error;
    }
    log.info('Side question was admitted before its reply was lost', {
      childSessionId: params.childSessionId,
      requestId,
    });
    return { ok: true };
  }));
  // Readback never retries an admitted turn, even if this optional read fails.
  // Fence against a subsequent user switch while the read is in flight.
  if (supportsBtwModelDefaults() && scope.isCurrent()) {
    try {
      const target = response.modelTarget ?? await modelPoolAPI.target({ sessionId: params.childSessionId,
        workspaceId: sessionWorkspaceId(childSession), workspacePath: childSession.workspacePath,
        remoteConnectionId: childSession.remoteConnectionId, remoteSshHost: childSession.remoteSshHost,
        includeInternal: true });
      const currentConfig = scope.isCurrent()
        ? flowChatStore.getState().sessions.get(params.childSessionId)?.config : undefined;
      if (currentConfig && currentConfig.modelName === submittedConfig.modelName
        && currentConfig.modelPoolId === submittedConfig.modelPoolId
        && currentConfig.reasoningPreset === submittedConfig.reasoningPreset
        && currentConfig.btwModelSelectionExplicit === submittedConfig.btwModelSelectionExplicit) {
        flowChatStore.updateSessionModelPool(params.childSessionId,
          target.selection.kind === 'pool' ? target.selection.pool_id : undefined, target.binding);
        flowChatStore.updateSessionModelName(params.childSessionId,
          target.selection.kind === 'model' ? target.selection.model_ref : target.binding?.modelId);
        flowChatStore.updateSessionReasoningPreset(params.childSessionId, target.reasoningPreset);
      }
    } catch (error) {
      if (scope.isCurrent()) log.warn('Could not read back the admitted BTW model selection', { error });
    }
  }
  followBtwSessionOnPeer(scope, params.childSessionId);
  if (scope.isCurrent() && params.initialModelTarget?.selection.kind === 'pool') {
    const current = flowChatStore.getState().sessions.get(params.childSessionId);
    if (current?.config.modelPoolId === params.initialModelTarget.selection.pool_id) {
      // The first turn already committed this choice. Follow-ups must not reset
      // the accepted member by replaying the draft selection.
      flowChatStore.updateSessionModelPool(params.childSessionId, current.config.modelPoolId,
        current.config.modelPoolBinding);
    }
  }
  if (modelId && !supportsBtwModelDefaults() && scope.isCurrent()) {
    flowChatStore.updateSessionModelName(params.childSessionId, modelId);
  }

  return { requestId };
}

export async function startBtwThread(params: {
  parentSessionId: string;
  workspacePath: string;
  question: string;
  modelId?: string;
  imagePayload?: ImagePayload;
}): Promise<{ requestId: string; childSessionId: string }> {
  const question = params.question.trim();
  if (!question) {
    notificationService.warning('Please provide a question after /btw');
    throw new Error('Empty /btw question');
  }

  await flowChatManager.ensureBackendSession(params.parentSessionId);
  const childSessionName = buildChildSessionName(question);
  const { childSessionId, parentDialogTurnId, parentTurnIndex } = createBtwSessionPlaceholder({
    parentSessionId: params.parentSessionId,
    workspacePath: params.workspacePath,
    childSessionName,
  });

  try {
    const { requestId } = await sendMessageToBtwSession({
      parentSessionId: params.parentSessionId,
      childSessionId,
      question,
      childSessionName,
      modelId: params.modelId,
      imagePayload: params.imagePayload,
      parentDialogTurnId,
      parentTurnIndex,
    });
    return { requestId, childSessionId };
  } catch (error) {
    flowChatManager.discardLocalSession(childSessionId);
    throw error;
  }
}
