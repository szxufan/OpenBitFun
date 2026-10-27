import { useEffect, useSyncExternalStore } from 'react';
import { relayAccountService } from './RelayAccountService';

/** Observe the relay-local account session (SMS/password sign-in). */
export function useAccountIdentity() {
  const snapshot = useSyncExternalStore(
    listener => relayAccountService.subscribe(listener),
    () => relayAccountService.getSnapshot(),
    () => relayAccountService.getSnapshot(),
  );

  useEffect(() => {
    void relayAccountService.initialize();
  }, []);

  return snapshot;
}
