import { useEffect, useRef, useState } from 'react';
import type { Session } from '@/flow_chat/types/flow-chat';
import { isLocalWorkspaceSession } from '@/flow_chat/utils/sessionWorkspace';
import { MCPAPI, type MCPResource } from '@/infrastructure/api/service-api/MCPAPI';
import { getChatMcpCatalog } from '@/infrastructure/api/service-api/ChatMcpAPI';
import { getActiveSurfaceScope, isLocalSurface } from '@/infrastructure/peer-device/deviceSurface';
import { useI18n } from '@/infrastructure/i18n';
import { fileValue, type BoundValue } from './visualBindings';

function assertWorkspace(session: Session) {
  if (!isLocalWorkspaceSession(session, undefined) || session.remoteConnectionId || session.remoteSshHost ||
      (session.workspaceHostname && session.workspaceHostname !== 'localhost')) {
    throw Error('Workspace resource browsing unavailable');
  }
}

// The reader selects an advertised resource from the owner's catalog. The
// author cannot choose a server, call a tool, or change the selected resource.
export function LiveResourceBinding({
  session,
  interval,
  format,
  onData,
}: {
  session: Session;
  interval: number;
  format: 'json' | 'file';
  onData: (value: BoundValue) => void;
}) {
  const { t } = useI18n('flow-chat');
  const [servers, setServers] = useState<{ id: string; name: string }[]>([]);
  const [serverId, setServerId] = useState('');
  const [resources, setResources] = useState<MCPResource[]>([]);
  const [selected, setSelected] = useState('');
  const [state, setState] = useState<'idle' | 'loading' | 'choose' | 'empty' | 'failed' | 'connected'>(
    'idle',
  );
  const owner = useRef(getActiveSurfaceScope()).current;
  const active = useRef(0);
  const deliver = useRef(onData);
  deliver.current = onData;
  useEffect(
    () => () => {
      active.current++;
    },
    [],
  );
  async function discover() {
    const revision = ++active.current;
    setServerId('');
    setSelected('');
    setServers([]);
    setResources([]);
    setState('loading');
    try {
      owner.assertCurrent('discover visualization sources');
      assertWorkspace(session);
      // Local discovery includes resource-only servers. The native resource API
      // enforces the server's workspace visibility again on list and read.
      const catalog = isLocalSurface(owner.surfaceId)
        ? (await MCPAPI.getServers()).map((server) => ({ id: server.id, name: server.name }))
        : (
            await getChatMcpCatalog({
              modeId: session.mode || session.lastUserDialogMode || session.config?.agentType || 'Standard',
              workspaceId: session.workspaceId,
            })
          ).tools.map((tool) => ({ id: tool.serverId, name: tool.serverName }));
      if (!owner.isCurrent() || active.current !== revision) return;
      assertWorkspace(session);
      const available = [...new Map(catalog.map((server) => [server.id, server])).values()];
      setServers(available);
      setState(available.length ? 'choose' : 'empty');
    } catch {
      if (owner.isCurrent() && active.current === revision) setState('failed');
    }
  }
  async function chooseServer(id: string) {
    const revision = ++active.current;
    setServerId(id);
    setResources([]);
    setSelected('');
    setState('loading');
    if (!id) {
      setState('choose');
      return;
    }
    try {
      owner.assertCurrent('list visualization sources');
      assertWorkspace(session);
      const available = await MCPAPI.listResources({ serverId: id });
      if (!owner.isCurrent() || active.current !== revision) return;
      assertWorkspace(session);
      const readable = available.filter((r) => !r.size || r.size <= 256 * 1024);
      setResources(readable);
      setState(readable.length ? 'choose' : 'empty');
    } catch {
      if (owner.isCurrent() && active.current === revision) setState('failed');
    }
  }
  useEffect(() => {
    if (!selected || !serverId) return;
    const resource = resources.find((r) => r.uri === selected);
    if (!resource) return;
    let stopped = false;
    let timer: number;
    let last: string | undefined;
    async function read() {
      if (stopped || !owner.isCurrent()) return;
      try {
        assertWorkspace(session);
        const response = await MCPAPI.readResource({ serverId, resourceUri: selected });
        if (stopped || !owner.isCurrent()) return;
        assertWorkspace(session);
        const entry = response.contents.find((c) => c.uri === selected);
        const text = entry?.text ?? entry?.content;
        if (!entry) throw Error('Unavailable resource');
        let data: BoundValue | undefined;
        if (typeof text === 'string') {
          if (format === 'json') {
            if (new TextEncoder().encode(text).length > 32768) throw Error('Oversize data');
            data = { kind: 'json', value: JSON.parse(text) };
          } else data = fileValue(resource!.name, text);
        } else if (
          format === 'file' &&
          entry.blob &&
          entry.blob.length <= 349528 &&
          atob(entry.blob).length <= 256 * 1024
        ) {
          data = {
            kind: 'file',
            value: { name: resource!.name, base64: entry.blob, mimeType: 'application/octet-stream' },
          };
        }
        if (!data) throw Error('Unavailable resource content');
        const serialized = text ?? entry.blob;
        if (last !== serialized) {
          last = serialized;
          deliver.current(data);
        }
        setState('connected');
      } catch {
        if (!stopped && owner.isCurrent()) setState('failed');
      }
      if (!stopped && owner.isCurrent()) timer = window.setTimeout(read, interval * 1000);
    }
    void read();
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [selected, serverId, resources, interval, owner, format, session]);
  return (
    <div className="visual-answer__controls">
      {(!servers.length || state === 'failed') && (
        <button type="button" disabled={state === 'loading'} onClick={discover}>
          {t('visualize.connectResource')}
        </button>
      )}
      {servers.length > 0 && (
        <select
          aria-label={t('visualize.server')}
          value={serverId}
          onChange={(e) => void chooseServer(e.target.value)}
        >
          <option value="">{t('visualize.server')}</option>
          {servers.map((s) => (
            <option key={s.id} value={s.id}>
              {s.name}
            </option>
          ))}
        </select>
      )}
      {resources.length > 0 && (
        <select
          aria-label={t('visualize.resource')}
          value={selected}
          onChange={(e) => {
            setSelected(e.target.value);
            setState(e.target.value ? 'loading' : 'choose');
          }}
        >
          <option value="">{t('visualize.resource')}</option>
          {resources.map((r) => (
            <option key={r.uri} value={r.uri}>
              {r.title || r.name}
            </option>
          ))}
        </select>
      )}
      {state === 'failed' && <small role="status">{t('visualize.resourceUnavailable')}</small>}
      {state === 'empty' && <small role="status">{t('visualize.noResources')}</small>}
      {state === 'connected' && (
        <small role="status">{t('visualize.refreshInterval', { seconds: interval })}</small>
      )}
    </div>
  );
}
