/**
 * @vitest-environment jsdom
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  listJobs: vi.fn(),
  deleteJob: vi.fn(),
  onJobsChanged: vi.fn(() => () => {}),
  confirmDanger: vi.fn(),
  notifyError: vi.fn(),
}));

vi.mock('@openbitfun/ui', () => ({
  Icon: ({ name, glyph: _glyph }: { name?: string; glyph?: unknown }) => (
    <span data-openbitfun-component="icon" data-openbitfun-name={name} />
  ),
}));

vi.mock('@/infrastructure/i18n', () => ({
  useI18n: () => ({ t: (key: string) => key, formatDate: () => 'time' }),
}));

vi.mock('@/infrastructure/api', () => ({
  cronAPI: {
    listJobs: mocks.listJobs,
    deleteJob: mocks.deleteJob,
    onJobsChanged: mocks.onJobsChanged,
  },
}));

vi.mock('@/infrastructure/confirm-dialog', () => ({ confirmDanger: mocks.confirmDanger }));
vi.mock('@/shared/notification-system', () => ({ notificationService: { error: mocks.notifyError } }));

import ScheduledSendPendingStrip from './ScheduledSendPendingStrip';

interface JobOverrides {
  id?: string;
  schedule?: { kind: 'at'; at: string } | { kind: 'cron'; expr: string };
  enabled?: boolean;
  lastEnqueuedAtMs?: number | null;
  activeTurnId?: string | null;
}

function makeJob(overrides: JobOverrides = {}) {
  return {
    id: overrides.id ?? 'job-1',
    name: 'job',
    schedule: overrides.schedule ?? { kind: 'at' as const, at: new Date(2026, 0, 15, 20, 0).toISOString() },
    payload: { text: 'hello' },
    enabled: overrides.enabled ?? true,
    target: { kind: 'session' as const, sessionId: 's1', workspace: { workspaceId: 'w1' } },
    createdAtMs: 0,
    configUpdatedAtMs: 0,
    updatedAtMs: 0,
    state: {
      consecutiveFailures: 0,
      coalescedRunCount: 0,
      lastEnqueuedAtMs: overrides.lastEnqueuedAtMs ?? null,
      activeTurnId: overrides.activeTurnId ?? null,
    },
  };
}

describe('ScheduledSendPendingStrip', () => {
  let root: Root;
  let container: HTMLDivElement;

  beforeEach(() => {
    globalThis.IS_REACT_ACT_ENVIRONMENT = true;
    mocks.listJobs.mockReset();
    mocks.deleteJob.mockReset();
    mocks.confirmDanger.mockReset();
    mocks.notifyError.mockReset();
    mocks.onJobsChanged.mockReturnValue(() => {});
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it('renders only pending one-shot jobs for the session', async () => {
    mocks.listJobs.mockResolvedValue([
      makeJob({ id: 'pending' }),
      makeJob({ id: 'fired', lastEnqueuedAtMs: 123 }),
      makeJob({ id: 'recurring', schedule: { kind: 'cron', expr: '0 8 * * *' } }),
      makeJob({ id: 'disabled', enabled: false }),
    ]);

    await act(async () => root.render(<ScheduledSendPendingStrip sessionId="s1" />));
    await act(async () => {});

    const items = container.querySelectorAll('[data-openbitfun-part="scheduledSendItem"]');
    expect(items.length).toBe(1);
    expect(mocks.listJobs).toHaveBeenCalledWith({ sessionId: 's1' });
  });

  it('cancels a pending job after confirmation', async () => {
    mocks.listJobs.mockResolvedValue([makeJob({ id: 'pending' })]);
    mocks.confirmDanger.mockResolvedValue(true);
    mocks.deleteJob.mockResolvedValue(true);

    await act(async () => root.render(<ScheduledSendPendingStrip sessionId="s1" />));
    await act(async () => {});

    const cancel = container.querySelector('[data-testid="chat-input-scheduled-cancel-pending"]');
    expect(cancel).not.toBeNull();
    await act(async () => {
      (cancel as HTMLButtonElement).dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    await act(async () => {});

    expect(mocks.confirmDanger).toHaveBeenCalled();
    expect(mocks.deleteJob).toHaveBeenCalledWith('pending');
  });

  it('does not render when there is no session', async () => {
    await act(async () => root.render(<ScheduledSendPendingStrip sessionId={undefined} />));
    await act(async () => {});
    expect(container.querySelector('[data-testid="chat-input-scheduled-strip"]')).toBeNull();
  });
});
