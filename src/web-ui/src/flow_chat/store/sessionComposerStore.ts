import { create } from 'zustand';

import type { ContextItem } from '@/shared/types/context';
import {
  getActiveSurfaceId,
  surfaceScopedKey,
  type DeviceSurfaceId,
} from '@/infrastructure/peer-device/deviceSurface';

export type PendingLargePasteMap = Record<string, string>;
type ComposerPrefillSource = 'skin-design' | 'assistant-bootstrap';

export interface SessionComposerDraft {
  value: string;
  contexts: ContextItem[];
  pendingLargePastes: PendingLargePasteMap;
  prefillSource?: ComposerPrefillSource;
  updatedAt: number;
}

interface SessionComposerState {
  drafts: Record<string, SessionComposerDraft>;
  getDraft: (sessionId: string, surfaceId?: DeviceSurfaceId) => SessionComposerDraft;
  activateDraft: (
    previousSessionId: string | null,
    nextSessionId: string | null,
    currentContexts: ContextItem[],
    persistPreviousContexts?: boolean,
  ) => SessionComposerDraft;
  setValue: (sessionId: string, value: string, surfaceId?: DeviceSurfaceId) => void;
  setPrefill: (sessionId: string, value: string, source: ComposerPrefillSource, surfaceId: DeviceSurfaceId) => void;
  setContexts: (sessionId: string, contexts: ContextItem[], surfaceId?: DeviceSurfaceId) => void;
  setPendingLargePastes: (sessionId: string, pendingLargePastes: PendingLargePasteMap, surfaceId?: DeviceSurfaceId) => void;
  clearDraft: (sessionId: string) => void;
  removeDrafts: (sessionIds: Iterable<string>) => void;
  removeSurfaceDrafts: (surfaceId: DeviceSurfaceId) => void;
}

const EMPTY_CONTEXTS: ContextItem[] = [];
const EMPTY_PENDING_LARGE_PASTES: PendingLargePasteMap = {};
const pendingSubmissions = new Map<string, number>();

function createEmptyDraft(): SessionComposerDraft {
  return {
    value: '',
    contexts: EMPTY_CONTEXTS,
    pendingLargePastes: EMPTY_PENDING_LARGE_PASTES,
    updatedAt: 0,
  };
}

function draftKey(sessionId: string, surfaceId = getActiveSurfaceId()): string {
  return surfaceScopedKey(surfaceId, sessionId);
}

export function isComposerSubmissionPending(sessionId: string, surfaceId: DeviceSurfaceId): boolean {
  return pendingSubmissions.has(draftKey(sessionId, surfaceId));
}

/** Protect the cleared composer while a send prepares attachments and awaits admission. */
export function beginComposerSubmission(sessionId: string, surfaceId: DeviceSurfaceId): () => void {
  const key = draftKey(sessionId, surfaceId);
  pendingSubmissions.set(key, (pendingSubmissions.get(key) ?? 0) + 1);
  return () => {
    const remaining = (pendingSubmissions.get(key) ?? 1) - 1;
    if (remaining) pendingSubmissions.set(key, remaining);
    else pendingSubmissions.delete(key);
  };
}

function updateDraft(
  state: SessionComposerState,
  sessionId: string,
  update: Partial<Omit<SessionComposerDraft, 'updatedAt'>>,
  surfaceId = getActiveSurfaceId(),
): Pick<SessionComposerState, 'drafts'> {
  const key = draftKey(sessionId, surfaceId);
  const current = state.drafts[key] ?? createEmptyDraft();
  return {
    drafts: {
      ...state.drafts,
      [key]: {
        ...current,
        ...update,
        updatedAt: Date.now(),
      },
    },
  };
}

export const useSessionComposerStore = create<SessionComposerState>((set, get) => ({
  drafts: {},

  getDraft: (sessionId, surfaceId) => get().drafts[draftKey(sessionId, surfaceId)] ?? createEmptyDraft(),

  activateDraft: (
    previousSessionId,
    nextSessionId,
    currentContexts,
    persistPreviousContexts = true,
  ) => {
    if (
      persistPreviousContexts
      && previousSessionId
      && previousSessionId !== nextSessionId
    ) {
      get().setContexts(previousSessionId, currentContexts);
    }
    return nextSessionId ? get().getDraft(nextSessionId) : createEmptyDraft();
  },

  setValue: (sessionId, value, surfaceId) => {
    set(state => {
      const current = state.drafts[draftKey(sessionId, surfaceId)];
      // Passive restoration keeps the template marker; editing makes it user content.
      const prefillSource = current?.prefillSource === 'assistant-bootstrap' && current.value !== value
        ? undefined : current?.prefillSource;
      return updateDraft(state, sessionId, { value, prefillSource }, surfaceId);
    });
  },

  setPrefill: (sessionId, value, prefillSource, surfaceId) => {
    set(state => updateDraft(state, sessionId, { value, prefillSource }, surfaceId));
  },

  setContexts: (sessionId, contexts, surfaceId) => {
    set(state => updateDraft(state, sessionId, { contexts: [...contexts] }, surfaceId));
  },

  setPendingLargePastes: (sessionId, pendingLargePastes, surfaceId) => {
    set(state => updateDraft(state, sessionId, {
      pendingLargePastes: { ...pendingLargePastes },
    }, surfaceId));
  },

  clearDraft: (sessionId) => {
    set(state => {
      if (!state.drafts[draftKey(sessionId)]) {
        return state;
      }

      return updateDraft(state, sessionId, {
        value: '',
        contexts: [],
        pendingLargePastes: {},
        prefillSource: undefined,
      });
    });
  },

  removeDrafts: (sessionIds) => {
    const ids = new Set(Array.from(sessionIds, sessionId => draftKey(sessionId)));
    if (ids.size === 0) {
      return;
    }

    set(state => {
      const drafts = { ...state.drafts };
      let changed = false;
      ids.forEach(key => {
        if (key in drafts) {
          delete drafts[key];
          changed = true;
        }
      });
      return changed ? { drafts } : state;
    });
  },

  removeSurfaceDrafts: (surfaceId) => {
    set(state => {
      const drafts = { ...state.drafts };
      let changed = false;
      for (const key of Object.keys(drafts)) {
        try {
          const [ownedSurfaceId] = JSON.parse(key) as [DeviceSurfaceId, string];
          if (ownedSurfaceId === surfaceId) {
            delete drafts[key];
            changed = true;
          }
        } catch {
          // Ignore legacy/malformed in-memory keys; no persisted shape exists.
        }
      }
      return changed ? { drafts } : state;
    });
  },
}));

export const sessionComposerStore = useSessionComposerStore;
