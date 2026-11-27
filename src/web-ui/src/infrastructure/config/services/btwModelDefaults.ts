import { isTauriRuntime } from '@/infrastructure/runtime';
import { peerConnectionManager } from '@/infrastructure/peer-device/PeerConnectionManager';
import { getActiveSurfaceScope } from '@/infrastructure/peer-device/deviceSurface';
import type { AgentModelTargetSelection } from '../types';

export function supportsBtwModelDefaults(): boolean {
  const scope = getActiveSurfaceScope();
  return scope.surfaceId === 'local' ? isTauriRuntime() : peerConnectionManager.list()
    .find(peer => peer.surfaceId === scope.surfaceId)?.capabilities.btwModelDefaultsV1 === true;
}

/** Only an absent field means inherit; unreadable choices must stay visible. */
export function btwModelSelection(config: unknown): AgentModelTargetSelection | undefined {
  if (!config || typeof config !== 'object' || Array.isArray(config)) return undefined;
  const value = (config as Record<string, unknown>).model_selection;
  if (value === undefined) return { kind: 'inherit' };
  if (!value || typeof value !== 'object') return undefined;
  const choice = value as Record<string, unknown>;
  if (choice.kind === 'inherit') return { kind: 'inherit' };
  if (choice.kind === 'model' && typeof choice.model_ref === 'string' && choice.model_ref.trim()) {
    return { kind: 'model', model_ref: choice.model_ref };
  }
  if (choice.kind === 'pool' && typeof choice.pool_id === 'string' && choice.pool_id.trim()) {
    return { kind: 'pool', pool_id: choice.pool_id };
  }
  return undefined;
}
