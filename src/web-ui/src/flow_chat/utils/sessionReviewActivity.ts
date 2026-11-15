import { SessionExecutionState } from '../state-machine/types';
import type { DialogTurn, FlowChatState, Session } from '../types/flow-chat';

export type SessionReviewActivityKind = 'review' | 'deep_review';
export type SessionReviewActivityLifecycle =
  | 'running'
  | 'finishing'
  | 'completed'
  | 'cancelled'
  | 'error'
  | 'idle';

export interface SessionReviewActivity {
  parentSessionId: string;
  childSessionId: string;
  kind: SessionReviewActivityKind;
  lifecycle: SessionReviewActivityLifecycle;
  isBlocking: boolean;
  startedAt: number;
  updatedAt: number;
}

export type SessionExecutionStateResolver = (
  sessionId: string,
) => SessionExecutionState | undefined;

const BLOCKING_LIFECYCLES = new Set<SessionReviewActivityLifecycle>([
  'running',
  'finishing',
]);

function deriveLifecycleFromTurn(
  turn?: DialogTurn,
  session?: Session,
): SessionReviewActivityLifecycle {
  if (session?.error) {
    return 'error';
  }

  switch (turn?.status) {
    case 'pending':
    case 'image_analyzing':
    case 'processing':
      return 'running';
    case 'finishing':
      return 'finishing';
    case 'cancelled':
      return 'cancelled';
    case 'error':
      return 'error';
    case 'completed':
      return 'completed';
    default:
      return 'idle';
  }
}

function deriveLifecycle(
  session: Session,
  executionState?: SessionExecutionState,
): SessionReviewActivityLifecycle {
  if (session.error) {
    return 'error';
  }

  switch (executionState) {
    case SessionExecutionState.PROCESSING:
      return 'running';
    case SessionExecutionState.FINISHING:
      return 'finishing';
    case SessionExecutionState.ERROR:
      return 'error';
    case SessionExecutionState.IDLE:
    default:
      return deriveLifecycleFromTurn(
        session.dialogTurns[session.dialogTurns.length - 1],
        session,
      );
  }
}

function toReviewActivity(
  session: Session,
  parentSessionId: string,
  resolveExecutionState?: SessionExecutionStateResolver,
): SessionReviewActivity | null {
  const kind = session.sessionKind === 'deep_review'
    ? 'deep_review'
    : session.sessionKind === 'review'
      ? 'review'
      : null;
  if (
    !kind ||
    session.parentSessionId !== parentSessionId
  ) {
    return null;
  }

  const lifecycle = deriveLifecycle(
    session,
    resolveExecutionState?.(session.sessionId),
  );

  return {
    parentSessionId,
    childSessionId: session.sessionId,
    kind,
    lifecycle,
    isBlocking: BLOCKING_LIFECYCLES.has(lifecycle),
    startedAt: session.createdAt,
    updatedAt: session.lastActiveAt || session.updatedAt || session.createdAt,
  };
}

export function isReviewActivityBlocking(
  activity?: SessionReviewActivity | null,
): boolean {
  return Boolean(activity?.isBlocking);
}

export function isSameSessionReviewActivity(
  left: SessionReviewActivity | null,
  right: SessionReviewActivity | null,
): boolean {
  if (left === right) {
    return true;
  }
  if (!left || !right) {
    return false;
  }
  return (
    left.parentSessionId === right.parentSessionId &&
    left.childSessionId === right.childSessionId &&
    left.kind === right.kind &&
    left.lifecycle === right.lifecycle &&
    left.isBlocking === right.isBlocking &&
    left.startedAt === right.startedAt &&
    left.updatedAt === right.updatedAt
  );
}

export function deriveSessionReviewActivity(
  state: FlowChatState,
  parentSessionId?: string | null,
  resolveExecutionState?: SessionExecutionStateResolver,
): SessionReviewActivity | null {
  if (!parentSessionId) {
    return null;
  }

  let bestBlocking: SessionReviewActivity | null = null;
  let bestAny: SessionReviewActivity | null = null;

  const isNewer = (candidate: SessionReviewActivity, current: SessionReviewActivity | null): boolean => {
    if (!current) return true;
    const updatedDelta = candidate.updatedAt - current.updatedAt;
    return updatedDelta !== 0 ? updatedDelta > 0 : candidate.startedAt > current.startedAt;
  };

  for (const session of state.sessions.values()) {
    const activity = toReviewActivity(session, parentSessionId, resolveExecutionState);
    if (!activity) {
      continue;
    }
    if (activity.isBlocking) {
      if (isNewer(activity, bestBlocking)) {
        bestBlocking = activity;
      }
    } else if (!bestBlocking && isNewer(activity, bestAny)) {
      bestAny = activity;
    }
  }

  return bestBlocking ?? bestAny;
}
