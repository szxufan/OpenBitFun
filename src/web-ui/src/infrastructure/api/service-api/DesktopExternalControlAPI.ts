import { listen } from '@tauri-apps/api/event';
import { api } from './ApiClient';

export type ControllerRequest =
  | { operation: 'status' }
  | { operation: 'setEnabled'; enabled: boolean; port?: number }
  | { operation: 'issue'; name: string; workspaceIds: string[]; origin?: string }
  | { operation: 'revoke'; id: string };

export interface DesktopControllerSettings {
  enabled: boolean;
  listening: boolean;
  port: number;
  endpoint: string;
  error: string | null;
  token: string | null;
  grants: { id: string; name: string; workspaceIds: string[]; origin: string | null }[];
  workspaces: { id: string; name: string }[];
}

export function desktopControllerSettings(request: ControllerRequest): Promise<DesktopControllerSettings> {
  return api.invoke('desktop_external_control', { request }, { retries: 0 });
}

export function listenForControllerSessionOpen(open: (sessionId: string, workspaceId: string) => void) {
  return listen<{ sessionId: string; workspaceId: string }>('desktop-control://open-session', ({ payload }) => {
    open(payload.sessionId, payload.workspaceId);
  });
}
