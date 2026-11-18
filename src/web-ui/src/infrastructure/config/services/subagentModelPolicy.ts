import { configAPI } from '@/infrastructure/api/service-api/ConfigAPI';
import { getActiveSurfaceScope } from '@/infrastructure/peer-device/deviceSurface';
import { peerConnectionManager } from '@/infrastructure/peer-device/PeerConnectionManager';
import { configManager } from './ConfigManager';

export const subagentModelOverridePath = 'ai.allow_subagent_model_override';

export function supportsSubagentModelPolicy(): boolean {
  const scope = getActiveSurfaceScope();
  return scope.surfaceId === 'local' || peerConnectionManager.list()
    .find(peer => peer.surfaceId === scope.surfaceId)?.capabilities.subagentModelOverridePolicyV1 === true;
}

/** Undefined means the connected host predates this policy, not a default of true. */
export async function readSubagentModelPolicy(): Promise<boolean | undefined> {
  if (!supportsSubagentModelPolicy()) return undefined;
  const value = await configAPI.getConfig(subagentModelOverridePath);
  if (value == null) return undefined;
  if (typeof value !== 'boolean') throw new Error('Invalid subagent model override policy');
  return value;
}

export async function writeSubagentModelPolicy(allowed: boolean): Promise<void> {
  if (!supportsSubagentModelPolicy()) throw new Error('Subagent model override policy is unsupported on this host');
  await configManager.setConfig(subagentModelOverridePath, allowed);
}
