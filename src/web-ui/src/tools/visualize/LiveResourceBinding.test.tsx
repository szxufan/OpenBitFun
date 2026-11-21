// @vitest-environment jsdom
import { act, type ReactElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, expect, it, vi } from 'vitest';
import type { Session } from '@/flow_chat/types/flow-chat';
import { LiveResourceBinding } from './LiveResourceBinding';

const host = vi.hoisted(() => ({
  servers: vi.fn(),
  list: vi.fn(),
  read: vi.fn(),
  current: vi.fn(),
}));
vi.mock('@/infrastructure/services/business/workspaceManager', () => ({
  workspaceManager: { getState: () => ({
    openedWorkspaces: new Map([['project', { id: 'project', workspaceKind: 'normal' }]]),
    recentWorkspaces: [{ id: 'ssh', workspaceKind: 'remote' }],
  }) },
}));
vi.mock('@/infrastructure/api/service-api/ChatMcpAPI', () => ({ getChatMcpCatalog: vi.fn() }));
vi.mock('@/infrastructure/i18n', () => ({ useI18n: () => ({ t: (s: string) => s }) }));
vi.mock('@/infrastructure/api/service-api/MCPAPI', () => ({
  MCPAPI: { getServers: host.servers, listResources: host.list, readResource: host.read },
}));
vi.mock('@/infrastructure/peer-device/deviceSurface', async importOriginal => ({
  ...await importOriginal<typeof import('@/infrastructure/peer-device/deviceSurface')>(),
  getActiveSurfaceScope: () => ({ surfaceId: 'local', isCurrent: host.current, assertCurrent: () => {} }),
  isLocalSurface: () => true,
}));
let root: Root | undefined;
function render(node: ReactElement) {
  const host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  act(() => root!.render(node));
  return host;
}
function select(host: HTMLElement, label: string, value: string) {
  const el = host.querySelector('select[aria-label="' + label + '"]') as HTMLSelectElement;
  el.value = value;
  el.dispatchEvent(new Event('change', { bubbles: true }));
}
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = undefined;
  document.body.replaceChildren();
  vi.useRealTimers();
  vi.resetAllMocks();
});
it('polls only a reader-selected resource, retries offline, and drops replies after a surface switch', async () => {
  vi.useFakeTimers();
  host.current.mockReturnValue(true);
  host.servers.mockResolvedValue([{ id: 'weather', name: 'Weather' }]);
  host.list.mockResolvedValue([{ uri: 'weather://daily', name: 'daily.json' }]);
  host.read
    .mockResolvedValueOnce({ contents: [{ uri: 'weather://daily', text: '{"temp":14}' }] })
    .mockRejectedValueOnce(Error('offline'))
    .mockResolvedValue({ contents: [{ uri: 'weather://daily', text: '{"temp":17}' }] });
  const received = vi.fn();
  const view = render(
    <LiveResourceBinding session={{ workspaceId: 'project' } as Session} format="json" interval={15} onData={received} />,
  );
  await act(async () => {
    view.querySelector('button')!.click();
  });
  expect(host.read).not.toHaveBeenCalled();
  await act(async () => {
    select(view, 'visualize.server', 'weather');
  });
  await act(async () => {
    select(view, 'visualize.resource', 'weather://daily');
  });
  expect(host.read).toHaveBeenCalledWith({ serverId: 'weather', resourceUri: 'weather://daily' });
  expect(received).toHaveBeenLastCalledWith({ kind: 'json', value: { temp: 14 } });
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(view.querySelector('[role=status]')?.textContent).toBe('visualize.resourceUnavailable');
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(received).toHaveBeenLastCalledWith({ kind: 'json', value: { temp: 17 } });
  host.current.mockReturnValue(false);
  await act(() => vi.advanceTimersByTimeAsync(30000));
  expect(host.read).toHaveBeenCalledTimes(3);
  act(() => root!.unmount());
  root = undefined;
  expect(vi.getTimerCount()).toBe(0);
});
it.each([{ workspaceId: 'ssh' }, { workspaceId: 'missing' }, {}, { workspaceId: 'project', remoteConnectionId: 'ssh-other' }])('requires an identified local workspace for discovery: %o', async (session) => {
  host.current.mockReturnValue(true);
  const view = render(
    <LiveResourceBinding
      session={session as Session}
      format="file"
      interval={15}
      onData={vi.fn()}
    />,
  );
  await act(async () => {
    view.querySelector('button')!.click();
  });
  expect(host.servers).not.toHaveBeenCalled();
  expect(view.querySelector('[role=status]')?.textContent).toBe('visualize.resourceUnavailable');
});
