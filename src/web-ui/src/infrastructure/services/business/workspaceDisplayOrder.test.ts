import { afterEach, describe, expect, it } from 'vitest';
import {
  clearPendingWorkspaceReveal,
  consumeWorkspaceReveal,
  getPendingWorkspaceReveal,
  getWorkspaceDisplayOrderRevision,
  orderWorkspacesForDisplay,
  prepareWorkspacePromotion,
  requestWorkspaceReveal,
  resetWorkspaceDisplayOrdersForTest,
  setWorkspaceDisplayOrder,
  subscribeWorkspacePromotionSnapshot,
  subscribeWorkspaceReveal,
} from './workspaceDisplayOrder';

const list = (...ids: string[]) => ids.map(id => ({ id }));
const ids = (items: Array<{ id: string }>) => items.map(item => item.id);

describe('workspaceDisplayOrder', () => {
  afterEach(resetWorkspaceDisplayOrdersForTest);

  it('orders cached lists by the latest recorded surface order without mutating state', () => {
    setWorkspaceDisplayOrder('peer', ['b', 'a']);
    const revision = getWorkspaceDisplayOrderRevision();

    expect(ids(orderWorkspacesForDisplay('peer', list('a', 'new', 'b')))).toEqual(['new', 'b', 'a']);
    expect(ids(orderWorkspacesForDisplay('other', list('a', 'b')))).toEqual(['a', 'b']);
    expect(getWorkspaceDisplayOrderRevision()).toBe(revision);
  });

  it('keeps device surfaces independent and bumps revision only when the order changes', () => {
    setWorkspaceDisplayOrder('local', ['a', 'b']);
    setWorkspaceDisplayOrder('peer', ['b', 'a']);
    const revision = getWorkspaceDisplayOrderRevision();

    setWorkspaceDisplayOrder('local', ['a', 'b']);
    expect(getWorkspaceDisplayOrderRevision()).toBe(revision);

    setWorkspaceDisplayOrder('local', ['b', 'a']);
    expect(getWorkspaceDisplayOrderRevision()).toBe(revision + 1);
    expect(ids(orderWorkspacesForDisplay('local', list('a', 'b')))).toEqual(['b', 'a']);
    expect(ids(orderWorkspacesForDisplay('peer', list('a', 'b')))).toEqual(['b', 'a']);
  });

  it('tracks promotion snapshots and per-surface reveal requests', () => {
    const events: string[] = [];
    const unsubscribeSnapshot = subscribeWorkspacePromotionSnapshot((surfaceId, workspaceId) => {
      events.push(`snapshot:${surfaceId}:${workspaceId}`);
    });
    const unsubscribeReveal = subscribeWorkspaceReveal(request => {
      events.push(`reveal:${request.surfaceId}:${request.workspaceId}:${request.promoted}`);
    });

    prepareWorkspacePromotion('local', 'proj-b');
    const request = requestWorkspaceReveal('local', 'proj-b', true);
    expect(events).toEqual([
      'snapshot:local:proj-b',
      'reveal:local:proj-b:true',
    ]);
    expect(getPendingWorkspaceReveal('local')).toEqual(request);

    consumeWorkspaceReveal('local', request.token);
    expect(getPendingWorkspaceReveal('local')).toBeUndefined();

    requestWorkspaceReveal('peer', 'proj-c', false);
    clearPendingWorkspaceReveal('peer');
    expect(getPendingWorkspaceReveal('peer')).toBeUndefined();

    unsubscribeSnapshot();
    unsubscribeReveal();
  });
});
