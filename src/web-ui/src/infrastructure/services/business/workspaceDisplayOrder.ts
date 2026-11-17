/**
 * Shared display-order and sidebar-reveal coordination per device surface.
 *
 * The backend `WorkspaceManager` owns workspace order: ordinary activation
 * preserves positions, while explicit folder opens place projects at the front
 * of the project section. On the frontend, the live rendered surface
 * (`WorkspaceManager`) and background device sections (`DeviceWorkspaceCatalog`)
 * read `get_opened_workspaces` at different times across device switches;
 * recording the latest authoritative order per surface lets cached hand-overs
 * render without moving rows.
 */

const orders = new Map<string, readonly string[]>();
const listeners = new Set<() => void>();
let revision = 0;

export interface WorkspaceRevealRequest {
  readonly surfaceId: string;
  readonly workspaceId: string;
  readonly promoted: boolean;
  readonly token: number;
}

const pendingReveals = new Map<string, WorkspaceRevealRequest>();
const promotionSnapshotListeners = new Set<(surfaceId: string, workspaceId: string) => void>();
const revealListeners = new Set<(request: WorkspaceRevealRequest) => void>();
const revealStateListeners = new Set<() => void>();
let revealRevision = 0;

const sameIds = (left: readonly string[], right: readonly string[]) => (
  left.length === right.length && left.every((id, index) => id === right[index])
);

export function setWorkspaceDisplayOrder(surfaceId: string, workspaceIds: readonly string[]): void {
  const previous = orders.get(surfaceId);
  orders.set(surfaceId, [...workspaceIds]);
  if (previous && sameIds(previous, workspaceIds)) return;
  revision += 1;
  for (const listener of Array.from(listeners)) listener();
}

/** Changes whenever a surface's display order changes (for `useSyncExternalStore`). */
export function getWorkspaceDisplayOrderRevision(): number {
  return revision;
}

export function subscribeWorkspaceDisplayOrder(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/**
 * Let mounted views snapshot pre-reorder row geometry before the promoted list
 * is published, without triggering external-store renders against stale state.
 */
export function prepareWorkspacePromotion(surfaceId: string, workspaceId: string): void {
  for (const listener of Array.from(promotionSnapshotListeners)) {
    listener(surfaceId, workspaceId);
  }
}

export function subscribeWorkspacePromotionSnapshot(
  listener: (surfaceId: string, workspaceId: string) => void,
): () => void {
  promotionSnapshotListeners.add(listener);
  return () => { promotionSnapshotListeners.delete(listener); };
}

/**
 * Announce that an explicit user open should reveal `workspaceId` in the
 * sidebar once the updated workspace state has been published.
 */
export function requestWorkspaceReveal(
  surfaceId: string,
  workspaceId: string,
  promoted: boolean,
): WorkspaceRevealRequest {
  revealRevision += 1;
  const request: WorkspaceRevealRequest = { surfaceId, workspaceId, promoted, token: revealRevision };
  pendingReveals.set(surfaceId, request);
  for (const listener of Array.from(revealListeners)) listener(request);
  for (const listener of Array.from(revealStateListeners)) listener();
  return request;
}

export function getPendingWorkspaceReveal(surfaceId: string): WorkspaceRevealRequest | undefined {
  return pendingReveals.get(surfaceId);
}

export function consumeWorkspaceReveal(surfaceId: string, token: number): void {
  if (pendingReveals.get(surfaceId)?.token === token) {
    pendingReveals.delete(surfaceId);
  }
}

export function clearPendingWorkspaceReveal(surfaceId: string): void {
  if (pendingReveals.delete(surfaceId)) {
    revealRevision += 1;
    for (const listener of Array.from(revealStateListeners)) listener();
  }
}

export function getWorkspaceRevealRevision(): number {
  return revealRevision;
}

export function subscribeWorkspaceReveal(listener: (request: WorkspaceRevealRequest) => void): () => void {
  revealListeners.add(listener);
  return () => { revealListeners.delete(listener); };
}

export function subscribeWorkspaceRevealState(listener: () => void): () => void {
  revealStateListeners.add(listener);
  return () => { revealStateListeners.delete(listener); };
}

/**
 * `workspaces` ordered by the surface's latest authoritative order, without
 * mutating stored state. Used during live/background cached hand-overs.
 */
export function orderWorkspacesForDisplay<T extends { id: string }>(surfaceId: string, workspaces: readonly T[]): T[] {
  const remembered = orders.get(surfaceId);
  if (!remembered) return [...workspaces];
  const byId = new Map(workspaces.map(workspace => [workspace.id, workspace]));
  const result: T[] = remembered.flatMap(id => (byId.has(id) ? [byId.get(id)!] : []));
  const placed = new Set(result.map(workspace => workspace.id));
  workspaces.forEach((workspace, index) => {
    if (placed.has(workspace.id)) return;
    const after = new Set(workspaces.slice(index + 1).map(candidate => candidate.id));
    const first = result.findIndex(candidate => after.has(candidate.id));
    result.splice(first === -1 ? result.length : first, 0, workspace);
    placed.add(workspace.id);
  });
  return result;
}

export function resetWorkspaceDisplayOrdersForTest(): void {
  orders.clear();
  pendingReveals.clear();
}
