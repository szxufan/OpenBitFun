// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import { PEER_CONTROLLER_LOCAL_COMMANDS } from '@/infrastructure/api/generated/remoteSurface';
import StorageSettingsPage from './StorageSettingsPage';

const mocks = vi.hoisted(() => ({ inspect: vi.fn(), clean: vi.fn(), confirm: vi.fn(), desktop: true }));
vi.mock('@/infrastructure/api/service-api/SnapshotStorageAPI', () => ({ snapshotStorageAPI: mocks }));
vi.mock('@/infrastructure/confirm-dialog', () => ({ confirmDanger: mocks.confirm }));
vi.mock('@/infrastructure/runtime', () => ({ isTauriRuntime: () => mocks.desktop }));
vi.mock('@/infrastructure/i18n', () => ({ i18nService: { formatNumber: String, formatDate: () => 'date' } }));
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));

globalThis.IS_REACT_ACT_ENVIRONMENT = true;
let root: Root;
let host: HTMLDivElement;
const project = { workspaceId: 'w', name: 'Project', location: '/p', bytes: 100, sharedBytes: 10,
  estimatedBytes: 90, skippedSessions: 0, warning: null, unverifiedReferences: false };
const candidate = { sessionId: 'old', name: 'Old session', activeAt: 20, revision: '10:20' };
beforeEach(() => {
  vi.resetAllMocks();
  mocks.desktop = true;
  mocks.inspect.mockResolvedValue({ scanErrors: [], totalBytes: 135, unidentifiedBytes: 35,
    workspaces: [{ ...project, candidates: [candidate] }] });
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
});
afterEach(() => { act(() => root.unmount()); host.remove(); });

it('requires confirmation and cleans only the previewed revision before refreshing usage', async () => {
  await act(async () => root.render(<StorageSettingsPage navigationRequestId={1} />));
  expect(host.textContent).toContain('35 B');
  const cleanup = () => [...host.querySelectorAll('button')].find(b => b.textContent === 'storage.cleanup')!;
  mocks.confirm.mockResolvedValueOnce(false);
  await act(async () => cleanup().click());
  expect(mocks.clean).not.toHaveBeenCalled();
  mocks.confirm.mockResolvedValueOnce(true);
  mocks.clean.mockResolvedValue({ removedSessions: 1, freedBytes: 90, skippedSessions: 0, errors: [] });
  await act(async () => cleanup().click());
  expect(mocks.clean).toHaveBeenCalledWith('w', 'thirtyDays', [candidate]);
  expect(mocks.inspect).toHaveBeenCalledTimes(2);
  expect(host.textContent).toContain('storage.result');
});

it('keeps storage on the desktop controller and reports unsupported browser surfaces', async () => {
  expect(PEER_CONTROLLER_LOCAL_COMMANDS.has('inspect_snapshot_storage')).toBe(true);
  expect(PEER_CONTROLLER_LOCAL_COMMANDS.has('clean_snapshot_storage')).toBe(true);
  mocks.desktop = false;
  await act(async () => root.render(<StorageSettingsPage navigationRequestId={1} />));
  expect(host.textContent).toContain('storage.desktopOnly');
  expect(mocks.inspect).not.toHaveBeenCalled();
});

it('shows workspace usage and expands candidate sessions independently by workspace', async () => {
  mocks.inspect.mockResolvedValue({
    totalBytes: 200, unidentifiedBytes: 0, scanErrors: [],
    workspaces: [
      { ...project, candidates: [candidate] },
      { ...project, workspaceId: 'other', name: 'Other', candidates: [{ ...candidate, name: 'Other session' }] },
    ],
  });
  await act(async () => root.render(<StorageSettingsPage navigationRequestId={1} />));
  expect(host.querySelectorAll('table tbody tr')).toHaveLength(2);
  expect(host.querySelector('tbody td')?.textContent).toBe('10 B');
  const groups = host.querySelectorAll<HTMLButtonElement>('button[aria-controls][aria-expanded]');
  const other = Array.from(groups).find(button => button.textContent === 'Other')!;
  expect(other.getAttribute('aria-expanded')).toBe('false');
  expect(host.textContent).not.toContain('Other session');
  await act(async () => other.click());
  expect(other.getAttribute('aria-expanded')).toBe('true');
  const content = document.getElementById(other.getAttribute('aria-controls')!);
  expect(content?.textContent).toContain('Other session');
  expect(content?.querySelector('time')?.dateTime).toBe(new Date(candidate.activeAt).toISOString());
  expect(host.textContent).not.toContain('Old session');

  const period = host.querySelector<HTMLSelectElement>('select[aria-label="storage.period"]')!;
  await act(async () => {
    period.value = 'sevenDays';
    period.dispatchEvent(new Event('change', { bubbles: true }));
  });
  expect(mocks.inspect).toHaveBeenLastCalledWith('sevenDays');
  const refreshedGroups = host.querySelectorAll('[data-openbitfun-part="candidateList"] button[aria-expanded]');
  expect(Array.from(refreshedGroups, button => button.getAttribute('aria-expanded'))).toEqual(['false', 'false']);
});

it('explains an empty preview and disables cleanup', async () => {
  mocks.inspect.mockResolvedValue({
    totalBytes: 100, unidentifiedBytes: 0, scanErrors: [],
    workspaces: [{ ...project, candidates: [], estimatedBytes: 0, skippedSessions: 1 }],
  });
  await act(async () => root.render(<StorageSettingsPage navigationRequestId={1} />));
  expect(host.textContent).toContain('storage.noCandidates');
  const cleanup = Array.from(host.querySelectorAll('button')).find(button => button.textContent === 'storage.cleanup');
  expect(cleanup?.disabled).toBe(true);
});

it('identifies removed-session snapshots without showing a fabricated session activity date', async () => {
  const removed = { ...candidate, removed: true, name: '', revision: 'removed:20' };
  mocks.inspect.mockResolvedValue({
    totalBytes: 100, unidentifiedBytes: 0, scanErrors: [],
    workspaces: [{ ...project, candidates: [removed] }],
  });
  await act(async () => root.render(<StorageSettingsPage navigationRequestId={1} />));
  const group = host.querySelector<HTMLButtonElement>('[data-openbitfun-part="candidateList"] button')!;
  expect(group.textContent).toBe(project.name);
  expect(group.getAttribute('aria-expanded')).toBe('false');
  await act(async () => group.click());
  const row = host.querySelector('[data-openbitfun-part="session"]')!;
  expect(row.textContent).toContain(removed.sessionId);
  expect(row.textContent).toContain('storage.removedSession');
  expect(row.querySelector('time')).toBeNull();
});
