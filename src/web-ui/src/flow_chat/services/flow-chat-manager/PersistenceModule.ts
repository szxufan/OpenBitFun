import type { SessionLastTurn } from '@/shared/types/session-history';
import { requireSessionOwningWorkspaceId } from '../../utils/sessionOrdering';
/**
 * Persistence module
 * Handles persistence operations for dialog turn saving and metadata management
 */

import { createLogger } from '@/shared/utils/logger';
import type { UiSessionMetadataField } from '@/infrastructure/api/service-api/SessionAPI';
import { getActiveSurfaceScope, isLocalSurface } from '@/infrastructure/peer-device/deviceSurface';
import type { FlowChatContext, DialogTurn } from './types';
import { buildSessionMetadata } from '../../utils/sessionMetadata';
import { settleInterruptedDialogTurn } from '../../utils/dialogTurnStability';
import {
  DEFERRED_TOOL_GATEWAY_NAME,
  effectiveToolInvocation,
} from '../../utils/toolInvocationIdentity';
import { resolveSessionDriverId } from '../../session-drivers/resolve';
import { resolveStorageTurnIndex } from '../../utils/flowChatTurnIdentity';
import { sessionActivityStore } from '../../store/sessionActivityStore';

const log = createLogger('PersistenceModule');
const COALESCED_IMMEDIATE_SAVE_DELAY_MS = 500;

function isTransientSession(session: { isTransient?: boolean; draft?: { phase: string } } | undefined): boolean {
  return session?.isTransient === true || session?.draft?.phase === 'editing' || session?.draft?.phase === 'creating';
}

/**
 * Observer projections are target-owned; the controller must never persist
 * them as local sessions. Uses the driver resolver so a projection whose
 * config is not bound yet (startup race) is still recognized via the
 * observer-store membership signal.
 */
function isObserverOnlyDispatchSession(
  sessionId: string,
  session: Parameters<typeof resolveSessionDriverId>[1],
): boolean {
  return resolveSessionDriverId(sessionId, session) === 'dispatch';
}

function getDialogTurn(context: FlowChatContext, sessionId: string, turnId: string): DialogTurn | undefined {
  return context.flowChatStore
    .getState()
    .sessions
    .get(sessionId)
    ?.dialogTurns
    .find(turn => turn.id === turnId);
}

function isTerminalDialogTurnStatus(status: DialogTurn['status']): boolean {
  return status === 'completed' || status === 'cancelled' || status === 'error';
}

function clearSaveTimer(context: FlowChatContext, key: string): void {
  const existingTimer = context.saveDebouncers.get(key);
  if (existingTimer) {
    clearTimeout(existingTimer);
    context.saveDebouncers.delete(key);
  }
}

async function runSerialDialogTurnSave(
  context: FlowChatContext,
  sessionId: string,
  turnId: string,
  strict = false
): Promise<void> {
  const scope = getActiveSurfaceScope();
  const key = `${sessionId}:${turnId}`;
  const existingTask = context.turnSaveInFlight.get(key);
  if (existingTask) {
    context.turnSavePending.add(key);
    await existingTask.catch(error => { if (strict) throw error; });
    if (strict) scope.assertCurrent();
    if (strict) await runSerialDialogTurnSave(context, sessionId, turnId, true);
    return;
  }

  const task = Promise.resolve().then(async () => {
    try {
      do {
        context.turnSavePending.delete(key);
        if (strict) scope.assertCurrent();
        await performSaveDialogTurnToDisk(context, sessionId, turnId, strict);
      } while (context.turnSavePending.has(key));
    } finally {
      if (context.turnSaveInFlight.get(key) === task) {
        context.turnSaveInFlight.delete(key);
        context.turnSavePending.delete(key);
      }
    }
  });

  context.turnSaveInFlight.set(key, task);
  await task;
}

const FNV_OFFSET_1 = 0x811c9dc5;
const FNV_PRIME_1 = 0x01000193;
const FNV_OFFSET_2 = 0x9e3779b9;
const FNV_PRIME_2 = 0x85ebca6b;
const MAX_HASH_STRING_SAMPLES = 64;
const MAX_HASH_OBJECT_DEPTH = 5;

const objectFingerprintCache = new WeakMap<object, readonly [number, number]>();

function mixUint32(h1: number, h2: number, value: number): [number, number] {
  const v = value | 0;
  const next1 = Math.imul(h1 ^ (v & 0xffff), FNV_PRIME_1);
  const next1Hi = Math.imul(next1 ^ ((v >>> 16) & 0xffff), FNV_PRIME_1);
  const next2 = Math.imul(h2 ^ v, FNV_PRIME_2);
  return [next1Hi >>> 0, (next2 ^ (next2 >>> 13)) >>> 0];
}

function mixStringSampled(h1: number, h2: number, text: string): [number, number] {
  const len = text.length;
  [h1, h2] = mixUint32(h1, h2, len);
  if (len === 0) return [h1, h2];
  if (len <= MAX_HASH_STRING_SAMPLES) {
    for (let i = 0; i < len; i += 1) {
      [h1, h2] = mixUint32(h1, h2, text.charCodeAt(i));
    }
    return [h1, h2];
  }
  // Sample prefix (16), suffix (16), and 32 evenly spaced interior code units
  // with zero string slicing or heap allocation.
  for (let i = 0; i < 16; i += 1) {
    [h1, h2] = mixUint32(h1, h2, text.charCodeAt(i));
  }
  const interiorSpan = len - 32;
  for (let s = 0; s < 32; s += 1) {
    const idx = 16 + Math.floor(((s + 0.5) * interiorSpan) / 32);
    [h1, h2] = mixUint32(h1, h2, text.charCodeAt(idx));
  }
  for (let i = len - 16; i < len; i += 1) {
    [h1, h2] = mixUint32(h1, h2, text.charCodeAt(i));
  }
  return [h1, h2];
}

function fingerprintValue(value: unknown, depth: number): readonly [number, number] {
  if (value === null) return [0x11, 0x22];
  if (value === undefined) return [0x33, 0x44];
  const kind = typeof value;
  if (kind === 'string') {
    return mixStringSampled(FNV_OFFSET_1, FNV_OFFSET_2, value as string);
  }
  if (kind === 'number') {
    const n = value as number;
    return Number.isFinite(n)
      ? mixUint32(FNV_OFFSET_1, FNV_OFFSET_2, Math.floor(n) ^ Math.floor(n / 0x100000000))
      : [0x55, 0x66];
  }
  if (kind === 'boolean') {
    return (value as boolean) ? [0x77, 0x88] : [0x99, 0xaa];
  }
  if (kind !== 'object') {
    return [0xbb, 0xcc];
  }

  const obj = value as object;
  const canCacheObject = depth === MAX_HASH_OBJECT_DEPTH;
  if (canCacheObject) {
    const cached = objectFingerprintCache.get(obj);
    if (cached) {
      return cached;
    }
  }

  let h1 = FNV_OFFSET_1;
  let h2 = FNV_OFFSET_2;

  if (Array.isArray(obj)) {
    [h1, h2] = mixUint32(h1, h2, obj.length);
    if (depth > 0) {
      for (let i = 0; i < obj.length; i += 1) {
        const [c1, c2] = fingerprintValue(obj[i], depth - 1);
        [h1, h2] = mixUint32(h1, h2, c1);
        [h1, h2] = mixUint32(h1, h2, c2);
      }
    }
  } else {
    let keyCount = 0;
    for (const key in obj) {
      if (!Object.prototype.hasOwnProperty.call(obj, key)) continue;
      keyCount += 1;
      [h1, h2] = mixStringSampled(h1, h2, key);
      if (depth > 0) {
        const [c1, c2] = fingerprintValue((obj as Record<string, unknown>)[key], depth - 1);
        [h1, h2] = mixUint32(h1, h2, c1);
        [h1, h2] = mixUint32(h1, h2, c2);
      }
    }
    [h1, h2] = mixUint32(h1, h2, keyCount);
  }

  const pair: readonly [number, number] = [h1, h2];
  if (canCacheObject) {
    objectFingerprintCache.set(obj, pair);
  }
  return pair;
}

type TurnModelRound = DialogTurn['modelRounds'][number];
const settledRoundFingerprintCache = new WeakMap<TurnModelRound, readonly [number, number]>();

function fingerprintModelRound(round: TurnModelRound): readonly [number, number] {
  const isSettled = !round.isStreaming && Boolean(round.isComplete);
  if (isSettled) {
    const cached = settledRoundFingerprintCache.get(round);
    if (cached) {
      return cached;
    }
  }
  let r1 = FNV_OFFSET_1;
  let r2 = FNV_OFFSET_2;
  [r1, r2] = mixStringSampled(r1, r2, round.id);
  [r1, r2] = mixStringSampled(r1, r2, round.status || '');
  [r1, r2] = mixUint32(r1, r2, (round.isStreaming ? 1 : 0) | (round.isComplete ? 2 : 0));
  [r1, r2] = mixUint32(r1, r2, round.endTime ?? 0);
  [r1, r2] = mixUint32(r1, r2, round.attemptCount ?? 0);
  [r1, r2] = mixStringSampled(r1, r2, round.effectiveModelName || '');
  [r1, r2] = mixUint32(r1, r2, round.items.length);
  for (let i = 0; i < round.items.length; i += 1) {
    const [i1, i2] = fingerprintValue(round.items[i], MAX_HASH_OBJECT_DEPTH);
    [r1, r2] = mixUint32(r1, r2, i1);
    [r1, r2] = mixUint32(r1, r2, i2);
  }
  if (round.attemptDiagnostics?.length) {
    const [d1, d2] = fingerprintValue(round.attemptDiagnostics, 3);
    [r1, r2] = mixUint32(r1, r2, d1);
    [r1, r2] = mixUint32(r1, r2, d2);
  }
  const pair: readonly [number, number] = [r1, r2];
  if (isSettled) {
    settledRoundFingerprintCache.set(round, pair);
  }
  return pair;
}

/**
 * Calculate content hash for dialog turn (for deduplication).
 * Uses zero-allocation structural fingerprinting + WeakMap memoization on
 * immutable round items/results instead of full-turn JSON.stringify.
 */
export function calculateTurnHash(dialogTurn: DialogTurn): string {
  let h1 = FNV_OFFSET_1;
  let h2 = FNV_OFFSET_2;

  [h1, h2] = mixStringSampled(h1, h2, dialogTurn.status || '');
  [h1, h2] = mixUint32(h1, h2, dialogTurn.modelRounds.length);
  [h1, h2] = mixStringSampled(h1, h2, dialogTurn.error || '');
  [h1, h2] = mixUint32(h1, h2, dialogTurn.endTime ?? 0);

  if (dialogTurn.tokenUsage) {
    [h1, h2] = mixUint32(h1, h2, dialogTurn.tokenUsage.inputTokens ?? -1);
    [h1, h2] = mixUint32(h1, h2, dialogTurn.tokenUsage.outputTokens ?? -1);
    [h1, h2] = mixUint32(h1, h2, dialogTurn.tokenUsage.totalTokens ?? -1);
    [h1, h2] = mixUint32(h1, h2, dialogTurn.tokenUsage.cachedTokens ?? -1);
  }

  for (let r = 0; r < dialogTurn.modelRounds.length; r += 1) {
    const [r1, r2] = fingerprintModelRound(dialogTurn.modelRounds[r]);
    [h1, h2] = mixUint32(h1, h2, r1);
    [h1, h2] = mixUint32(h1, h2, r2);
  }

  return `${h1.toString(36)}_${h2.toString(36)}`;
}

const syncedUiSessionMetadataByContext = new WeakMap<FlowChatContext, Map<string, string>>();

function getSyncedUiMetadataMap(context: FlowChatContext): Map<string, string> {
  let map = syncedUiSessionMetadataByContext.get(context);
  if (!map) {
    map = new Map<string, string>();
    syncedUiSessionMetadataByContext.set(context, map);
  }
  return map;
}

function isPeerProjectedNonAcpSession(session: any): boolean {
  if (isLocalSurface(getActiveSurfaceScope().surfaceId)) {
    return false;
  }
  const provider = session?.config?.customMetadata?.sessionProvider ?? session?.customMetadata?.sessionProvider;
  return provider !== 'acp';
}

function computeUiMetadataSyncFingerprint(session: any): string {
  const [f1, f2] = fingerprintValue(
    {
      title: session?.title ?? '',
      titleSource: session?.titleSource ?? '',
      titleKey: session?.titleKey ?? '',
      titleParams: session?.titleParams ?? null,
      externalPlaceholder: Boolean(session?.externalPlaceholder),
      needsUserAttention: String(session?.needsUserAttention ?? ''),
      hasUnreadCompletion: String(session?.hasUnreadCompletion ?? ''),
      tags: session?.tags ?? null,
      todos: session?.todos ?? null,
    },
    3,
  );
  return `${f1.toString(36)}_${f2.toString(36)}`;
}

/**
 * Coalesce frequent dialog-turn updates into periodic latest-state saves.
 *
 * This intentionally behaves like a trailing throttle, not a pure debounce:
 * continuous model output must still reach persistence so Peer Device
 * snapshot recovery can advance while a turn is running.
 */
export function debouncedSaveDialogTurn(
  context: FlowChatContext,
  sessionId: string,
  turnId: string,
  delay: number = 2000
): void {
  const session = context.flowChatStore.getState().sessions.get(sessionId);
  if (session && isPeerProjectedNonAcpSession(session)) {
    return;
  }
  const key = `${sessionId}:${turnId}`;
  
  const existingTimer = context.saveDebouncers.get(key);
  if (existingTimer) {
    return;
  }
  
  const timer = setTimeout(() => {
    context.saveDebouncers.delete(key);
    saveDialogTurnToDisk(context, sessionId, turnId).catch(error => {
      log.warn('Coalesced checkpoint save failed', { sessionId, turnId, error });
    });
  }, delay);
  
  context.saveDebouncers.set(key, timer);
}

/**
 * Immediately save dialog turn (skip debounce)
 * Used for critical moments like round completion, tool execution completion, etc.
 */
export function immediateSaveDialogTurn(
  context: FlowChatContext,
  sessionId: string,
  turnId: string,
  skipDuplicateCheck: boolean = false
): void {
  const session = context.flowChatStore.getState().sessions.get(sessionId);
  if (session && isPeerProjectedNonAcpSession(session)) {
    return;
  }
  const key = `${sessionId}:${turnId}`;
  const dialogTurn = getDialogTurn(context, sessionId, turnId);
  
  if (!skipDuplicateCheck) {
    if (session) {
      if (dialogTurn) {
        const currentHash = calculateTurnHash(dialogTurn);
        const lastHash = context.lastSaveHashes.get(key);
        const lastTimestamp = context.lastSaveTimestamps.get(key) || 0;
        const now = Date.now();
        
        if (lastHash === currentHash && (now - lastTimestamp) < 5000) {
          return;
        }
        
        context.lastSaveHashes.set(key, currentHash);
        context.lastSaveTimestamps.set(key, now);
      }
    }
  }

  const shouldFlushImmediately =
    skipDuplicateCheck ||
    !dialogTurn ||
    isTerminalDialogTurnStatus(dialogTurn.status);

  if (shouldFlushImmediately) {
    clearSaveTimer(context, key);
    saveDialogTurnToDisk(context, sessionId, turnId).catch(error => {
      log.warn('Immediate save failed', { sessionId, turnId, error });
    });
    return;
  }

  clearSaveTimer(context, key);
  const timer = setTimeout(() => {
    context.saveDebouncers.delete(key);
    saveDialogTurnToDisk(context, sessionId, turnId).catch(error => {
      log.warn('Coalesced save failed', { sessionId, turnId, error });
    });
  }, COALESCED_IMMEDIATE_SAVE_DELAY_MS);
  context.saveDebouncers.set(key, timer);
}

/**
 * Clean up session save state
 * Called when session or turn is deleted
 */
export function cleanupSaveState(
  context: FlowChatContext,
  sessionId: string,
  turnId?: string
): void {
  if (turnId) {
    const key = `${sessionId}:${turnId}`;
    const timer = context.saveDebouncers.get(key);
    if (timer) {
      clearTimeout(timer);
      context.saveDebouncers.delete(key);
    }
    context.lastSaveTimestamps.delete(key);
    context.lastSaveHashes.delete(key);
    context.turnSavePending.delete(key);
    context.deferredStorageIdentitySaves?.delete(key);
    context.turnSaveInFlight.delete(key);
  } else {
    const keysToDelete = new Set<string>();
    for (const key of context.saveDebouncers.keys()) {
      if (key.startsWith(`${sessionId}:`)) {
        const timer = context.saveDebouncers.get(key);
        if (timer) {
          clearTimeout(timer);
        }
        keysToDelete.add(key);
      }
    }
    for (const key of context.lastSaveTimestamps.keys()) {
      if (key.startsWith(`${sessionId}:`)) {
        keysToDelete.add(key);
      }
    }
    for (const key of context.lastSaveHashes.keys()) {
      if (key.startsWith(`${sessionId}:`)) {
        keysToDelete.add(key);
      }
    }
    for (const key of context.turnSavePending.values()) {
      if (key.startsWith(`${sessionId}:`)) {
        keysToDelete.add(key);
      }
    }
    for (const key of context.turnSaveInFlight.keys()) {
      if (key.startsWith(`${sessionId}:`)) {
        keysToDelete.add(key);
      }
    }
    for (const key of context.deferredStorageIdentitySaves ?? []) {
      if (key.startsWith(`${sessionId}:`)) {
        keysToDelete.add(key);
      }
    }

    keysToDelete.forEach(key => {
      context.saveDebouncers.delete(key);
      context.lastSaveTimestamps.delete(key);
      context.lastSaveHashes.delete(key);
      context.turnSavePending.delete(key);
      context.deferredStorageIdentitySaves?.delete(key);
      context.turnSaveInFlight.delete(key);
    });
    const syncedUiSessionMetadataKeys = syncedUiSessionMetadataByContext.get(context);
    if (syncedUiSessionMetadataKeys) {
      for (const metaKey of syncedUiSessionMetadataKeys.keys()) {
        if (metaKey.endsWith(`:${sessionId}`)) {
          syncedUiSessionMetadataKeys.delete(metaKey);
        }
      }
    }
  }
}

/**
 * Save dialog turn to disk (FlowChat format → backend format)
 */
export async function saveDialogTurnToDisk(
  context: FlowChatContext,
  sessionId: string,
  turnId: string
): Promise<void> {
  clearSaveTimer(context, `${sessionId}:${turnId}`);
  await runSerialDialogTurnSave(context, sessionId, turnId);
}

/** Await a real durable write; unsupported owners and IO failures reject. */
export async function saveDialogTurnConfirmed(context: FlowChatContext, sessionId: string, turnId: string): Promise<void> {
  clearSaveTimer(context, sessionId + ':' + turnId);
  await runSerialDialogTurnSave(context, sessionId, turnId, true);
}

async function performSaveDialogTurnToDisk(
  context: FlowChatContext,
  sessionId: string,
  turnId: string,
  strict = false
): Promise<void> {
  const scope = getActiveSurfaceScope();
  const skip = (reason: string) => { if (strict) throw new Error(reason); };
  try {
    const { sessionAPI } = await import('@/infrastructure/api/service-api/SessionAPI');

    scope.assertCurrent();
    context.materializeStagedSession?.(sessionId, { silent: true, flushBatcher: true });
    const session = context.flowChatStore.getState().sessions.get(sessionId);
    if (!session) {
      skip('GenUI session is unavailable');
      log.debug('Session not found, skipping save', { sessionId, turnId });
      return;
    }
    if (isTransientSession(session) || isObserverOnlyDispatchSession(sessionId, session)) {
      skip('This session does not support controller-owned state persistence');
      return;
    }

    
    const dialogTurn = session.dialogTurns.find(turn => turn.id === turnId);
    if (!dialogTurn) {
      skip('GenUI source turn is unavailable');
      log.debug('Dialog turn not found, skipping save', { sessionId, turnId });
      return;
    }
    if (dialogTurn.recovery) {
      skip('Runtime-owned recovered turns do not support GenUI state updates');
      // The Runtime owns the durable snapshot and generation CAS for an
      // interrupted/recovered turn. A frontend whole-turn save can otherwise
      // overwrite newly appended rounds or restore stale recovery metadata.
      log.debug('Runtime owns persistence for recoverable dialog turn', {
        sessionId,
        turnId,
        recoveryStatus: dialogTurn.recovery.status,
        executionGeneration: dialogTurn.recovery.executionGeneration,
      });
      return;
    }

    const turnIndex = resolveStorageTurnIndex(session, dialogTurn);
    if (turnIndex === undefined) {
      skip('GenUI source turn has no durable storage identity');
      context.deferredStorageIdentitySaves?.add(`${sessionId}:${turnId}`);
      log.debug('Dialog turn has no storage identity, deferring save', { sessionId, turnId });
      return;
    }
    const turnData = convertDialogTurnToBackendFormat(dialogTurn, turnIndex);
    await sessionAPI.saveSessionTurn(
      turnData,
      requireSessionOwningWorkspaceId(session));
    
    scope.assertCurrent();
    // PersistenceManager::save_dialog_turn on the host already updates
    // metadata.json activity/counts/last_turn atomically. Skip the extra
    // load+save metadata double-IPC on mid-turn checkpoints unless UI-owned
    // session metadata changed or the turn reached a terminal state.
    const syncedUiSessionMetadataKeys = getSyncedUiMetadataMap(context);
    const metaSyncKey = `${scope.surfaceId}:${sessionId}`;
    const uiMetaFingerprint = computeUiMetadataSyncFingerprint(session);
    const shouldSyncMetadata =
      strict ||
      isTerminalDialogTurnStatus(dialogTurn.status) ||
      syncedUiSessionMetadataKeys.get(metaSyncKey) !== uiMetaFingerprint;
    if (shouldSyncMetadata) {
      const persistedMetadata = await performUpdateSessionMetadata(context, sessionId);
      if (persistedMetadata && scope.isCurrent()) {
        syncedUiSessionMetadataKeys.set(metaSyncKey, uiMetaFingerprint);
      }
    }
    scope.assertCurrent();
    
  } catch (error) {
    log.error('Failed to save dialog turn', { sessionId, turnId, error });
    if (strict) throw error;
  }
}

/**
 * Save all in-progress dialog turns by settling them for persistence.
 * Call only when the app process is about to exit; hiding to tray/dock keeps the
 * app alive and settling here would clear in-memory "active" state (e.g. desktop pet bubbles).
 */
export async function saveAllInProgressTurns(context: FlowChatContext): Promise<void> {
  if (context.stagedSessionBatches && context.stagedSessionBatches.size > 0) {
    for (const stagedSessionId of [...context.stagedSessionBatches.keys()]) {
      context.materializeStagedSession?.(stagedSessionId, { silent: true, flushBatcher: true });
    }
  }
  const state = context.flowChatStore.getState();
  const savePromises: Promise<void>[] = [];
  
  for (const [sessionId, session] of state.sessions.entries()) {
    if (isTransientSession(session) || isObserverOnlyDispatchSession(sessionId, session)) {
      continue;
    }
    const lastTurn = session.dialogTurns[session.dialogTurns.length - 1];
    
    if (lastTurn) {
      const key = `${sessionId}:${lastTurn.id}`;
      const timer = context.saveDebouncers.get(key);
      if (timer) {
        clearTimeout(timer);
        context.saveDebouncers.delete(key);
      }
      
      if (
        lastTurn.status !== 'completed' &&
        lastTurn.status !== 'cancelled' &&
        lastTurn.status !== 'error'
      ) {
        const settledAt = Date.now();
        context.flowChatStore.updateDialogTurn(sessionId, lastTurn.id, turn =>
          settleInterruptedDialogTurn(turn, settledAt, {
            interruptionReason: 'app_restart',
          })
        );

        // An interrupted shutdown still needs a visible-result acknowledgement.
        if (sessionId !== state.activeSessionId) {
          context.flowChatStore.markSessionUnreadCompletion(sessionId, 'interrupted', lastTurn.id);
        }

        savePromises.push(
          saveDialogTurnToDisk(context, sessionId, lastTurn.id).catch(error => {
            log.error('Failed to save in-progress turn', { sessionId, turnId: lastTurn.id, error });
          })
        );

        // Also persist the unread completion metadata
        if (sessionId !== state.activeSessionId) {
          savePromises.push(
            updateSessionMetadata(context, sessionId).catch(error => {
              log.error('Failed to update session metadata for unread completion', { sessionId, error });
            })
          );
        }
      }
    }
  }
  
  await Promise.all(savePromises);
}

interface CachedBackendModelRound {
  turnId: string;
  roundIndex: number;
  backendRound: any;
}

const settledBackendRoundCache = new WeakMap<DialogTurn['modelRounds'][number], CachedBackendModelRound>();

function convertModelRoundToBackendFormat(
  round: DialogTurn['modelRounds'][number],
  turnId: string,
  roundIndex: number,
): any {
  const isSettled = round.isStreaming === false && round.isComplete === true;
  if (isSettled) {
    const cached = settledBackendRoundCache.get(round);
    if (cached && cached.turnId === turnId && cached.roundIndex === roundIndex) {
      return cached.backendRound;
    }
  }

  const textItems: any[] = [];
  const toolItems: any[] = [];
  const thinkingItems: any[] = [];

  for (let index = 0; index < round.items.length; index += 1) {
    const item = round.items[index];
    if (item.type === 'text') {
      const textItem = item as any;
      textItems.push({
        id: item.id,
        content: textItem.content || '',
        isStreaming: textItem.isStreaming || false,
        isMarkdown: textItem.isMarkdown !== undefined ? textItem.isMarkdown : true,
        timestamp: item.timestamp,
        status: item.status || 'completed',
        orderIndex: index,
        subagentSessionId: textItem.subagentSessionId,
        attemptId: item.attemptId,
        attemptIndex: item.attemptIndex,
      });
    } else if (item.type === 'tool') {
      const toolItem = item as any;
      const effective = effectiveToolInvocation(toolItem.toolName, toolItem.toolCall?.input);
      if (
        toolItem.toolName === DEFERRED_TOOL_GATEWAY_NAME
        && toolItem.status === 'completed'
        && !effective.isDeferred
      ) {
        throw new Error(`Completed deferred tool is missing its wire invocation: ${item.id}`);
      }
      toolItems.push({
        id: item.id,
        toolName: toolItem.toolName || '',
        interruptionReason: toolItem.interruptionReason,
        toolCall: toolItem.toolCall || { input: {}, id: item.id },
        toolResult: toolItem.toolResult,
        aiIntent: toolItem.aiIntent,
        requiresConfirmation: toolItem.requiresConfirmation,
        userConfirmed: toolItem.userConfirmed,
        acpPermission: toolItem.acpPermission,
        startTime: toolItem.startTime || item.timestamp,
        endTime: toolItem.endTime,
        status: item.status || 'completed',
        orderIndex: index,
        subagentSessionId: toolItem.subagentSessionId,
        subagentDialogTurnId: toolItem.subagentDialogTurnId,
        subagentModelId: toolItem.subagentModelId,
        subagentModelDisplayName: toolItem.subagentModelDisplayName,
        attemptId: item.attemptId,
        attemptIndex: item.attemptIndex,
      });
    } else if (item.type === 'thinking') {
      const thinkingItem = item as any;
      thinkingItems.push({
        id: item.id,
        content: thinkingItem.content || '',
        isStreaming: thinkingItem.isStreaming || false,
        isCollapsed: thinkingItem.isCollapsed || false,
        timestamp: item.timestamp,
        status: item.status || 'completed',
        orderIndex: index,
        subagentSessionId: thinkingItem.subagentSessionId,
        attemptId: item.attemptId,
        attemptIndex: item.attemptIndex,
      });
    }
  }

  const backendRound = {
    id: round.id,
    turnId,
    roundIndex,
    roundGroupId: round.roundGroupId,
    timestamp: round.startTime,
    renderHints: round.renderHints,
    modelConfigId: round.modelConfigId,
    effectiveModelName: round.effectiveModelName,
    routing: round.routing,
    textItems,
    toolItems,
    thinkingItems,
    startTime: round.startTime,
    endTime: round.endTime,
    durationMs: round.durationMs,
    firstChunkMs: round.firstChunkMs,
    firstVisibleOutputMs: round.firstVisibleOutputMs,
    streamDurationMs: round.streamDurationMs,
    ttftMs: round.ttftMs,
    attemptCount: round.attemptCount,
    attemptDiagnostics: round.attemptDiagnostics,
    status: round.status || 'completed',
  };

  if (isSettled) {
    settledBackendRoundCache.set(round, { turnId, roundIndex, backendRound });
  }
  return backendRound;
}

/**
 * Convert FlowChat DialogTurn to backend format
 */
export function convertDialogTurnToBackendFormat(dialogTurn: DialogTurn, turnIndex: number): any {
  const userMetadata = dialogTurn.userMessage.metadata
    ? { ...dialogTurn.userMessage.metadata }
    : undefined;
  const mergedUserMetadata =
    dialogTurn.userMessage.images?.length
      ? {
          ...(userMetadata || {}),
          images: dialogTurn.userMessage.images.map(img => ({
            id: img.id,
            name: img.name,
            data_url: img.dataUrl,
            image_path: img.imagePath,
            mime_type: img.mimeType,
          })),
          original_text: dialogTurn.userMessage.content,
        }
      : userMetadata;

  return {
    turnId: dialogTurn.id,
    turnIndex,
    sessionId: dialogTurn.sessionId,
    timestamp: dialogTurn.startTime,
    kind: dialogTurn.kind || 'user_dialog',
    userMessage: {
      id: dialogTurn.userMessage.id,
      content: dialogTurn.userMessage.content,
      timestamp: dialogTurn.userMessage.timestamp,
      metadata: mergedUserMetadata,
    },
    modelRounds: dialogTurn.modelRounds.map((round, roundIndex) =>
      convertModelRoundToBackendFormat(round, dialogTurn.id, roundIndex),
    ),
    startTime: dialogTurn.startTime,
    endTime: dialogTurn.endTime,
    tokenUsage: dialogTurn.tokenUsage
      ? {
          inputTokens: dialogTurn.tokenUsage.inputTokens,
          outputTokens: dialogTurn.tokenUsage.outputTokens,
          ...(dialogTurn.tokenUsage.cachedTokens !== undefined
            ? { cachedTokens: dialogTurn.tokenUsage.cachedTokens } : {}),
          totalTokens: dialogTurn.tokenUsage.totalTokens,
          timestamp: dialogTurn.tokenUsage.timestamp,
        }
      : undefined,
    finishReason: dialogTurn.finishReason,
    recovery: dialogTurn.recovery,
    recoveryEpoch: dialogTurn.recovery?.executionGeneration ?? dialogTurn.recoveryEpoch,
    hasFinalResponse: dialogTurn.hasFinalResponse,
    error: dialogTurn.error,
    errorDetail: dialogTurn.errorDetail,
    status: dialogTurn.status === 'completed' ? 'completed' : 
            dialogTurn.status === 'error' ? 'error' : 
            dialogTurn.status === 'cancelled' ? 'cancelled' : 'inprogress',
  };
}

/**
 * Update session metadata (lastActiveAt, statistics, etc.)
 * Loads existing metadata first to avoid overwriting correct historical counts
 * when the in-memory dialogTurns only has a partial view (e.g. remote-triggered turns
 * on a persisted session whose full turn history hasn't been loaded yet).
 */
export async function updateSessionMetadata(
  context: FlowChatContext,
  sessionId: string,
  fields?: UiSessionMetadataField[],
  options?: { unreadReceipt?: SessionLastTurn },
): Promise<void> {
  await performUpdateSessionMetadata(context, sessionId, fields, options);
}

async function performUpdateSessionMetadata(
  context: FlowChatContext,
  sessionId: string,
  fields?: UiSessionMetadataField[],
  options?: { unreadReceipt?: SessionLastTurn },
): Promise<boolean> {
  const scope = getActiveSurfaceScope();
  // Runtime checkpoints already update host activity and counts. A peer
  // controller writes presentation metadata only for an explicit UI change.
  if (!fields && !isLocalSurface(scope.surfaceId)) return true;
  try {
    const { sessionAPI } = await import('@/infrastructure/api/service-api/SessionAPI');
    if (!scope.isCurrent()) return false;

    const session = context.flowChatStore.getState().sessions.get(sessionId);
    if (!session) return false;
    if (isTransientSession(session) || isObserverOnlyDispatchSession(sessionId, session)) return true;


    let existingMetadata: any = null;
    try {
      if (!fields) {
        existingMetadata = await sessionAPI.loadSessionMetadata(
          sessionId,
          requireSessionOwningWorkspaceId(session));
      }
    } catch (error) {
      // A full checkpoint merges into the stored record. Without it (an
      // unreachable remote workspace, a transient read failure) the write
      // would reset tags and title facts; the next checkpoint retries.
      log.debug('Skipped session metadata checkpoint: stored record unreadable', { sessionId, error });
      return false;
    }
    if (!scope.isCurrent()) return false;

    const metadata = buildSessionMetadata(session, existingMetadata);
    if (fields?.includes('unreadCompletion')) {
      const activity = sessionActivityStore.get(sessionId)?.summary;
      // lastTurn is a receipt guard in a UI patch, not a write to the host's
      // execution summary. A newer quiet run must not change its target.
      // The host accepts an acknowledgement only for its own record of the
      // result, so that receipt wins over one rebuilt from (possibly unloaded)
      // turns.
      const receipt = options?.unreadReceipt ?? activity?.unreadTurn;
      if (receipt) {
        metadata.lastTurn = receipt;
        metadata.customMetadata = { ...(metadata.customMetadata ?? {}), unreadCompletionTarget: receipt };
      }
    }

    // A remote-driven placeholder knows only a guessed title; until it adopts
    // the host's record it must not rename the persisted session.
    const defaultFields: UiSessionMetadataField[] = session.externalPlaceholder
      ? ['tags', 'todos', 'unreadCompletion', 'needsUserAttention']
      : ['sessionName', 'tags', 'todos', 'unreadCompletion', 'needsUserAttention', 'titleMetadata'];
    await sessionAPI.saveSessionMetadata(
      metadata,
      requireSessionOwningWorkspaceId(session),
      fields ?? defaultFields);
    return true;
  } catch (error) {
    log.warn('Failed to update session metadata', { sessionId, error });
    return false;
  }
}

/**
 * Update session activity time (used for session switching)
 */
export async function touchSessionActivity(
  sessionId: string,
  workspaceId: string
): Promise<void> {
  try {
    const { sessionAPI } = await import('@/infrastructure/api/service-api/SessionAPI');
    await sessionAPI.touchSessionActivity(
      sessionId,
      workspaceId
    );
  } catch (error) {
    log.debug('Failed to touch session activity', { sessionId, error });
  }
}
