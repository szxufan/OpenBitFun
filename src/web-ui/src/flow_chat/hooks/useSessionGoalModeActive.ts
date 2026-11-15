import { useSyncExternalStore } from 'react';
import { flowChatStore } from '../store/FlowChatStore';

const subscribeToFlowChatStore = (callback: () => void): (() => void) =>
  flowChatStore.subscribe(() => callback());

export function useSessionGoalModeActive(sessionId: string | undefined): boolean {
  return useSyncExternalStore(
    subscribeToFlowChatStore,
    () => (
      sessionId
        ? Boolean(flowChatStore.getState().sessions.get(sessionId)?.goalModeActive)
        : false
    ),
    () => false,
  );
}
