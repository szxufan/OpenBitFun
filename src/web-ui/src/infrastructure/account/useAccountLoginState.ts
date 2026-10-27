import { useDeviceDirectory, resolveDeviceName } from './deviceDirectory';
/** UI sign-in state follows the relay account, independently of Relay availability. */
import { useEffect, useState } from 'react';
import { useAccountIdentity } from '@/infrastructure/account-identity';
import { remoteConnectAPI } from '@/infrastructure/api/service-api/RemoteConnectAPI';

export interface AccountLoginState {
  loggedIn: boolean;
  deviceName: string | null;
}

export function useAccountLoginState(): AccountLoginState {
  const identity = useAccountIdentity();
  const directory = useDeviceDirectory();
  const accountId = identity.me?.user.id;
  const loggedIn = identity.status === 'signed-in' && accountId !== undefined;
  const [device, setDevice] = useState<{ accountId: string; name: string | null } | null>(null);
  useEffect(() => {
    if (!loggedIn || accountId === undefined) return;
    let current = true;
    void remoteConnectAPI.getDeviceInfo().then(info => {
      if (current) setDevice({ accountId, name: info.device_name || null });
    }).catch(() => {
      if (current) setDevice({ accountId, name: null });
    });
    return () => { current = false; };
  }, [loggedIn, accountId]);
  return { loggedIn, deviceName: loggedIn && directory.localId ? resolveDeviceName(directory.localId) : loggedIn && device?.accountId === accountId ? device.name : null };
}
