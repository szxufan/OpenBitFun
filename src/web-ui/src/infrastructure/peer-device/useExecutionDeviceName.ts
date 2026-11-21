import { useEffect, useState, useSyncExternalStore } from 'react';
import { resolveDeviceNameFrom, useDeviceDirectory } from '@/infrastructure/account/deviceDirectory';
import { remoteConnectAPI } from '@/infrastructure/api/service-api/RemoteConnectAPI';
import { getActiveSurfaceScope, isLocalSurface, onSurfaceActivated } from './deviceSurface';
import { usePeerDeviceModeOptional } from './peerDeviceContextState';

/**
 * This machine's own name. `remote_connect_get_device_info` stays on the
 * controller, works signed out, and does not change while the app runs, so
 * one read is shared. A failed read is retried by the next consumer.
 */
let localDeviceName: Promise<string | null> | null = null;

function loadLocalDeviceName(): Promise<string | null> {
  localDeviceName ??= remoteConnectAPI.getDeviceInfo()
    .then(info => info.device_name?.trim() || null)
    .catch(() => {
      localDeviceName = null;
      return null;
    });
  return localDeviceName;
}

/**
 * Name of the device that runs the rendered surface's sessions: this machine
 * on the local surface, the controlled device on a peer surface. Account
 * aliases win over host names. `null` until a name is known, so callers keep
 * a generic label instead of showing an id.
 */
export function useExecutionDeviceName(): string | null {
  const scope = useSyncExternalStore(onSurfaceActivated, getActiveSurfaceScope, getActiveSurfaceScope);
  const directory = useDeviceDirectory();
  const peerMode = usePeerDeviceModeOptional()?.peerMode;
  const [hostName, setHostName] = useState<string | null>(null);
  const local = isLocalSurface(scope.surfaceId);

  useEffect(() => {
    if (!local) return;
    let cancelled = false;
    void loadLocalDeviceName().then(name => { if (!cancelled) setHostName(name); });
    return () => { cancelled = true; };
  }, [local]);

  if (local) {
    return directory.localId
      ? resolveDeviceNameFrom(directory.devices, directory.localId, hostName ?? '') || hostName
      : hostName;
  }
  const announced = peerMode?.active && peerMode.deviceId === scope.surfaceId ? peerMode.deviceName : '';
  return resolveDeviceNameFrom(directory.devices, scope.surfaceId, announced) || null;
}

export function resetExecutionDeviceNameForTest(): void {
  localDeviceName = null;
}
