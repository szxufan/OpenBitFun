import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ContextItem } from '@/shared/types/context';
import { activateSurface, resetDeviceSurfaceForTest } from '@/infrastructure/peer-device/deviceSurface';
import { SessionExecutionState, stateMachineManager } from '../state-machine';
import { beginComposerSubmission, sessionComposerStore } from '../store/sessionComposerStore';
import type { FlowChatState, Session, SessionConfig } from '../types/flow-chat';
import { pendingQueueManager } from './flow-chat-manager/PendingQueueModule';
import { findReusableSessionDraft } from './sessionDraftReuse';

function session(overrides: Partial<Session> = {}): Session {
  return {
    sessionId: 'draft', workspaceId: 'workspace', workspacePath: '/repo',
    mode: 'Standard', config: {}, dialogTurns: [], status: 'idle', error: null,
    createdAt: 1, lastActiveAt: 1, historyState: 'new', sessionKind: 'normal',
    draft: { workspaceId: 'workspace', phase: 'editing', turnId: 'first' },
    ...overrides,
  };
}

function find(sessions: Session[], activeSessionId: string | null = null, config: SessionConfig = {}, mode = 'Standard') {
  const state: FlowChatState = {
    sessions: new Map(sessions.map(value => [value.sessionId, value])), activeSessionId,
  };
  return findReusableSessionDraft(state, 'workspace', mode, config);
}

beforeEach(() => {
  resetDeviceSurfaceForTest();
  sessionComposerStore.setState({ drafts: {} });
});
afterEach(() => {
  vi.restoreAllMocks();
  pendingQueueManager.clearSurface('local');
  pendingQueueManager.clearSurface('peer');
  resetDeviceSurfaceForTest();
});

describe('blank conversation reuse', () => {
  it('prefers the active blank draft, then the most recently active one', () => {
    const older = session({ sessionId: 'older' });
    const newer = session({ sessionId: 'newer', lastActiveAt: 20 });
    expect(find([older, newer], older.sessionId)).toBe('older');
    expect(find([older, newer], 'unrelated')).toBe('newer');
  });

  it.each(['Standard', 'Claw'])('reuses only the exact requested %s mode', mode => {
    const draft = session({ mode });
    expect(find([draft], null, {}, mode)).toBe('draft');
    expect(find([draft], null, {}, 'Cowork')).toBeNull();
  });

  it('uses workspace IDs even when two SSH hosts share the same path', () => {
    expect(find([session({ workspaceId: 'other-host', remoteConnectionId: 'ssh-b' })])).toBeNull();
    expect(find([session({ workspaceId: undefined, workspacePath: '/repo' })])).toBeNull();
    expect(find([session({ remoteConnectionId: 'ssh-a' })], null, {
      workspaceId: 'workspace', workspacePath: '/repo', remoteConnectionId: 'ssh-a',
    })).toBe('draft');
  });

  it('does not select a draft whose composer now targets another directory', () => {
    expect(find([session({ draft: { workspaceId: 'other', phase: 'editing', turnId: 'first' } })])).toBeNull();
  });

  it.each([
    { value: 'unfinished request' },
    { contexts: [{ id: 'file', type: 'file' } as ContextItem] },
    { pendingLargePastes: { placeholder: 'full text' } },
  ])('preserves occupied composers: %j', contents => {
    sessionComposerStore.setState({ drafts: {
      '["local","draft"]': { value: '', contexts: [], pendingLargePastes: {}, updatedAt: 1, ...contents },
    } });
    expect(find([session()])).toBeNull();
  });

  it('reuses an untouched assistant bootstrap prefill, but preserves user edits', () => {
    const composer = sessionComposerStore.getState();
    const claw = session({ mode: 'Claw' });
    composer.setPrefill('draft', 'Initialize my assistant', 'assistant-bootstrap', 'local');
    expect(find([claw], null, {}, 'Claw')).toBe('draft');
    composer.setValue('draft', 'Initialize my assistant with my own instructions');
    expect(find([claw], null, {}, 'Claw')).toBeNull();
  });

  it('checks composer occupancy on the selected device only', () => {
    sessionComposerStore.getState().setValue('draft', 'Local content');
    expect(find([session()])).toBeNull();
    activateSurface('peer');
    expect(find([session()])).toBe('draft');
    sessionComposerStore.getState().setValue('draft', 'Peer content');
    activateSurface('local');
    expect(sessionComposerStore.getState().getDraft('draft').value).toBe('Local content');
  });

  it('does not reuse a cleared composer while attachments are being submitted', () => {
    const finish = beginComposerSubmission('draft', 'local');
    try { expect(find([session()])).toBeNull(); } finally { finish(); }
    expect(find([session()])).toBe('draft');
  });

  it('preserves queued messages and running state even before a projected turn appears', () => {
    pendingQueueManager.enqueue({ sessionId: 'draft', content: 'Waiting' });
    expect(find([session()])).toBeNull();
    pendingQueueManager.clear('draft');
    vi.spyOn(stateMachineManager, 'getCurrentState').mockReturnValue(SessionExecutionState.PROCESSING);
    expect(find([session()])).toBeNull();
  });

  it.each<Partial<Session>>([
    { draft: undefined, isHistorical: true, historyState: 'metadata-only' },
    { historyState: 'hydrating' },
    { historyState: 'failed' },
    { persistedTurnCount: 2 },
    { totalTurnCount: 2, isPartial: true },
    { lastSubmittedMode: 'Standard' },
    { sessionKind: 'subagent', parentSessionId: 'parent' },
    { isTransient: true },
    { externalHistory: { mode: 'follow' } },
    { mode: 'acp:client', config: { agentType: 'acp:client' } },
    { config: { dispatchJobId: 'job' } },
  ])('does not treat existing or special sessions as blank drafts: %j', overrides => {
    expect(find([session(overrides)], null, {}, overrides.mode ?? 'Standard')).toBeNull();
  });

  it.each(['creating', 'ready', 'submitting'] as const)('keeps a %s draft on its own recovery path', phase => {
    expect(find([session({ draft: { workspaceId: 'workspace', phase, turnId: 'first' } })])).toBeNull();
  });

  it('honors explicit model, reasoning and context choices', () => {
    const config = { modelName: 'chosen', reasoningPreset: 'high', context: { key: 'value' } };
    const draft = session({ config });
    expect(find([draft], null, config)).toBe('draft');
    expect(find([draft], null, { modelName: 'other' })).toBeNull();
    expect(find([draft], null, { reasoningPreset: 'low' })).toBeNull();
    expect(find([draft], null, { context: { key: 'other' } })).toBeNull();
    expect(find([draft], null, { executionTargetRequest: { kind: 'local' } })).toBeNull();
  });
});
