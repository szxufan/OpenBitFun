/**
 * Capability answers for the host the window currently renders, for services
 * that run outside React. Components use `usePeerDeviceModeOptional()` with the
 * same resolvers from `peerCapabilityResolution`.
 */
import { getActiveSurfaceScope, isLocalSurface } from './deviceSurface';
import { isPeerDeviceModeActive } from './peerModeFlag';
import { peerConnectionManager, type PeerHostCapabilities } from './PeerConnectionManager';
import {
  canForkSessionsOnSurface,
  canInterruptRecoverablyOnSurface,
} from './peerCapabilityResolution';

function renderedHost(): { peerActive: boolean; capabilities: PeerHostCapabilities | null } {
  const { surfaceId } = getActiveSurfaceScope();
  const peerActive = isPeerDeviceModeActive() || !isLocalSurface(surfaceId);
  if (!peerActive) return { peerActive, capabilities: null };
  const peer = peerConnectionManager.list().find(entry => entry.surfaceId === surfaceId);
  return { peerActive, capabilities: peer?.capabilities ?? null };
}

/** Whether the rendered host can branch a session at a persisted turn. */
export function renderedHostCanForkSessions(): boolean {
  const { peerActive, capabilities } = renderedHost();
  return canForkSessionsOnSurface(peerActive, capabilities);
}

/** Whether Stop may interrupt recoverably instead of hard-cancelling. */
export function renderedHostInterruptsRecoverably(): boolean {
  const { peerActive, capabilities } = renderedHost();
  return canInterruptRecoverablyOnSurface(peerActive, capabilities);
}
