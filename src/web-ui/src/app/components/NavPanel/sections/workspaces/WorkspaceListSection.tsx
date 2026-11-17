import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useI18n } from '@/infrastructure/i18n';
import { useWorkspaceContext } from '@/infrastructure/contexts/WorkspaceContext';
import {
  consumeWorkspaceReveal,
  getPendingWorkspaceReveal,
  getWorkspaceRevealRevision,
  subscribeWorkspacePromotionSnapshot,
  subscribeWorkspaceRevealState,
} from '@/infrastructure/services/business/workspaceDisplayOrder';
import { useSceneStore } from '@/app/stores/sceneStore';
import { useContentResourceStore } from '@/app/workbench/contentResourceStore';
import { getActiveSurfaceId, onSurfaceActivated } from '@/infrastructure/peer-device/deviceSurface';
import { notificationService } from '@/shared/notification-system';
import WorkspaceItem from './WorkspaceItem';
import { setWorkspaceSessionsCollapsed, useWorkspaceSessionsCollapsed } from './workspaceDisclosure';
import { remoteWorkspaceBadges } from './remoteWorkspaceBadge';
import SessionViewTransition from './SessionViewTransition';
import SessionsSection, { type WorkspaceSessionScope } from '../sessions/SessionsSection';
import { useWorkspaceSessionViewStore } from '../../workspaceSessionView';
import {
  isWorkspaceBackedSessionGroupActive,
  isWorkspaceBackedSessionGroupSelected,
  projectWorkspaceBackedSessionGroups,
  type SessionNavigationScope,
  type WorkspaceBackedSessionGroup,
} from '../../sessionNavigationProjection';
import './WorkspaceListSection.scss';

interface WorkspaceListSectionProps {
  variant: SessionNavigationScope;
  scrollRootRef?: React.RefObject<HTMLElement | null>;
  /** Several devices share the sidebar: the flat list shows three rows per device. */
  limitFlatRows?: boolean;
}

type WorkspaceDragPosition = 'before' | 'after';

interface WorkspaceDragPayload {
  workspaceId: string;
  variant: SessionNavigationScope;
}

interface PendingDropSnapshot {
  surfaceId: string;
  scrollTop: number;
  rects: Map<string, DOMRect>;
}

const WORKSPACE_DRAG_MIME_TYPE = 'application/x-openbitfun-workspace';
const WORKSPACE_DROP_SETTLE_MS = 160;
const WORKSPACE_DROP_EASING = 'cubic-bezier(0.22, 1, 0.36, 1)';
const WORKSPACE_FLIP_VIEWPORT_MARGIN_PX = 40;
const WORKSPACE_FLIP_MAX_DELTA_PX = 240;

/**
 * Scroll `row`'s header card into the visible portion of `scrollRoot`, clearing
 * any sticky section header (which also clips covered content via `clip-path`).
 * Falls back to native `scrollIntoView` when no measured scroll container exists.
 */
function revealWorkspaceRow(row: HTMLElement, scrollRoot?: HTMLElement | null): void {
  const header = row.querySelector<HTMLElement>('.openbitfun-nav-panel__workspace-item-card') ?? row;
  if (scrollRoot) {
    const scrollRect = scrollRoot.getBoundingClientRect();
    const stickyHeader = row.closest('[data-openbitfun-section="sessions"]')
      ?.querySelector<HTMLElement>('[data-openbitfun-part="stickySectionHeader"]');
    const visibleTop = Math.max(scrollRect.top, stickyHeader?.getBoundingClientRect().bottom ?? scrollRect.top);
    const visibleBottom = scrollRect.bottom;
    if (visibleBottom > visibleTop) {
      const rowRect = row.getBoundingClientRect();
      const headerRect = header.getBoundingClientRect();
      if (headerRect.top < visibleTop) {
        scrollRoot.scrollTop = Math.max(0, scrollRoot.scrollTop + (rowRect.top - visibleTop));
        return;
      }
      if (headerRect.bottom > visibleBottom) {
        const delta = Math.min(headerRect.bottom - visibleBottom, rowRect.top - visibleTop);
        if (delta > 0) scrollRoot.scrollTop += delta;
        return;
      }
      return;
    }
  }
  header.scrollIntoView?.({ block: 'nearest' });
}

/**
 * WorkspaceListSection — the session section's group list.
 *
 * Projects and assistants are both workspace-backed, but they are not peers in
 * navigation: the assistant identity is pinned above the workspace list and
 * never joins the workspace order (see `pinnedGroups`).
 */

/** A workspace row whose project disclosure lives in the window-wide store. */
const ProjectWorkspaceItem: React.FC<React.ComponentProps<typeof WorkspaceItem> & {
  surfaceId: string;
  isProject: boolean;
}> = ({ surfaceId, isProject, ...props }) => {
  const collapsed = useWorkspaceSessionsCollapsed(surfaceId, props.workspace.id);
  return (
    <WorkspaceItem
      {...props}
      sessionsCollapsed={isProject ? collapsed : undefined}
      onSessionsCollapsedChange={isProject
        ? value => setWorkspaceSessionsCollapsed(surfaceId, props.workspace.id, value)
        : undefined}
    />
  );
};

const WorkspaceListSection: React.FC<WorkspaceListSectionProps> = ({ variant, scrollRootRef, limitFlatRows = false }) => {
  const { t } = useI18n('common');
  const {
    openedWorkspacesList,
    normalWorkspacesList,
    assistantWorkspacesList,
    activeWorkspaceId,
    reorderOpenedWorkspacesInSection,
  } = useWorkspaceContext();
  // Remote badges are compared across every opened workspace so equal compact labels expand.
  const remoteBadges = useMemo(() => remoteWorkspaceBadges(openedWorkspacesList), [openedWorkspacesList]);
  const [draggedWorkspaceId, setDraggedWorkspaceId] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<{
    workspaceId: string;
    position: WorkspaceDragPosition;
  } | null>(null);
  const grouping = useWorkspaceSessionViewStore(state => state.grouping);
  const activeTab = useSceneStore(state => state.openTabs.find(tab => tab.id === state.activeTabId));
  const contentScope = useContentResourceStore(state => (
    activeTab?.contentId ? state.resources[activeTab.contentId]?.scope : undefined
  ));
  const surfaceId = useSyncExternalStore(onSurfaceActivated, getActiveSurfaceId, getActiveSurfaceId);
  const revealRevision = useSyncExternalStore(
    subscribeWorkspaceRevealState,
    getWorkspaceRevealRevision,
    getWorkspaceRevealRevision,
  );

  // Refs for values that must be read inside event handlers without stale closures
  const draggedWorkspaceIdRef = useRef<string | null>(null);
  const dropTargetRef = useRef<{ workspaceId: string; position: WorkspaceDragPosition } | null>(null);
  const pendingDragEndRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const workspaceRowRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const pendingDropSnapshotRef = useRef<PendingDropSnapshot | null>(null);
  const dropAnimationsRef = useRef<Map<string, Animation>>(new Map());

  const sectionWorkspaces = variant === 'all'
    ? openedWorkspacesList
    : variant === 'assistants'
      ? assistantWorkspacesList
      : normalWorkspacesList;
  const sessionGroups = useMemo(() => {
    return projectWorkspaceBackedSessionGroups(sectionWorkspaces, variant);
  }, [sectionWorkspaces, variant]);
  const workspaces = useMemo(
    () => sessionGroups.map(group => group.workspace),
    [sessionGroups],
  );
  const activeWorkspace = openedWorkspacesList.find(workspace => workspace.id === activeWorkspaceId);
  const emptyLabel = variant === 'assistants'
    ? t('nav.workspaces.emptyAssistants')
    : variant === 'projects'
      ? t('nav.workspaces.emptyProjects')
      : t('nav.workspaces.emptySessionGroups');
  const workspaceScopes = useMemo<WorkspaceSessionScope[]>(() => (
    variant !== 'assistants'
      ? sessionGroups.filter(group => group.kind === 'project').map(({ workspace }) => ({
          workspaceId: workspace.id,
          workspaceName: workspace.name,
        }))
      : []
  ), [variant, sessionGroups]);
  /**
   * The assistant is a product-level identity, not one workspace among many. It
   * is pinned above the workspace list and stays out of the workspace order, so
   * the persisted and draggable order only ever describes project workspaces.
   *
   * A hidden, deleted, or never-created assistant has no opened workspace and
   * therefore produces no pinned row: the pin appears and disappears with the
   * assistant itself, and the workspace list is untouched either way.
   */
  const pinnedGroups = variant === 'all'
    ? sessionGroups.filter(group => group.kind === 'assistant')
    : [];
  const reorderableGroups = variant === 'all'
    ? sessionGroups.filter(group => group.kind === 'project')
    : sessionGroups;
  // The flat view folds every project session into one aggregate list, so only
  // the pinned assistant rows stay above it.
  const visibleReorderableGroups = grouping === 'all' && variant !== 'assistants'
    ? []
    : reorderableGroups;
  const canReorderGroups = visibleReorderableGroups.length > 1;
  const canContinueDrag = draggedWorkspaceId !== null && canReorderGroups
    && visibleReorderableGroups.some(group => group.workspace.id === draggedWorkspaceId);
  /**
   * Pinned assistants sit outside the drag order, so a project drag rewrites
   * project positions only and leaves the assistant's stored slot untouched.
   */
  const reorderSection = variant === 'all' ? 'projects' : variant;

  const workspaceSignature = sessionGroups.map(group => group.groupId).join(':');

  const resetDragState = useCallback(() => {
    clearTimeout(pendingDragEndRef.current);
    pendingDragEndRef.current = undefined;
    draggedWorkspaceIdRef.current = null;
    dropTargetRef.current = null;
    setDraggedWorkspaceId(null);
    setDropTarget(null);
  }, []);

  // A retained list can lose its native source without receiving dragend.
  // Device identity matters even when the new surface has identical workspace IDs.
  // Pending reveal requests live in `workspaceDisplayOrder` so switching from
  // flat (`all`) to grouped still reveals the row once it mounts.
  useLayoutEffect(() => {
    resetDragState();
    pendingDropSnapshotRef.current = null;
  }, [grouping, surfaceId, variant, resetDragState]);

  const captureWorkspaceRowsSnapshot = useCallback((): PendingDropSnapshot => ({
    surfaceId,
    scrollTop: scrollRootRef?.current?.scrollTop ?? 0,
    rects: new Map(
      Array.from(workspaceRowRefs.current, ([id, element]) => (
        [id, element.getBoundingClientRect()] as const
      )),
    ),
  }), [scrollRootRef, surfaceId]);

  // Re-adding an opened workspace moves it to the top of this device's list.
  // Snapshot pre-reorder geometry before the new order commits so rows can slide.
  useEffect(() => subscribeWorkspacePromotionSnapshot((promotedSurfaceId, workspaceId) => {
    if (promotedSurfaceId !== surfaceId) return;
    if (!workspaceRowRefs.current.has(workspaceId)) return;
    pendingDropSnapshotRef.current = captureWorkspaceRowsSnapshot();
  }), [captureWorkspaceRowsSnapshot, surfaceId]);

  useLayoutEffect(() => {
    if (draggedWorkspaceId && !canContinueDrag) {
      resetDragState();
    }
  }, [canContinueDrag, draggedWorkspaceId, resetDragState]);

  useEffect(() => {
    if (!draggedWorkspaceId) return;

    const handleWindowDrop = () => {
      // Capture sees drops that other targets stop from bubbling. Let React's
      // target handler read the source and before/after slot before clearing them.
      clearTimeout(pendingDragEndRef.current);
      pendingDragEndRef.current = setTimeout(resetDragState, 0);
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') resetDragState();
    };
    const handleVisibilityChange = () => {
      if (document.visibilityState === 'hidden') resetDragState();
    };
    const handleMove = (event: MouseEvent) => {
      if (event.buttons === 0) resetDragState();
    };
    // HTML dragging itself emits pointercancel at startup; it is not an end
    // signal. Releases and resumed input recover a missed native dragend instead.
    const endEvents = ['dragend', 'mouseup', 'pointerup', 'mousedown', 'pointerdown'] as const;
    endEvents.forEach(type => window.addEventListener(type, resetDragState, true));
    window.addEventListener('drop', handleWindowDrop, true);
    window.addEventListener('keydown', handleKeyDown, true);
    window.addEventListener('mousemove', handleMove, true);
    window.addEventListener('pointermove', handleMove, true);
    window.addEventListener('blur', resetDragState);
    window.addEventListener('pagehide', resetDragState);
    document.addEventListener('visibilitychange', handleVisibilityChange);
    return () => {
      clearTimeout(pendingDragEndRef.current);
      pendingDragEndRef.current = undefined;
      endEvents.forEach(type => window.removeEventListener(type, resetDragState, true));
      window.removeEventListener('drop', handleWindowDrop, true);
      window.removeEventListener('keydown', handleKeyDown, true);
      window.removeEventListener('mousemove', handleMove, true);
      window.removeEventListener('pointermove', handleMove, true);
      window.removeEventListener('blur', resetDragState);
      window.removeEventListener('pagehide', resetDragState);
      document.removeEventListener('visibilitychange', handleVisibilityChange);
    };
  }, [draggedWorkspaceId, resetDragState]);

  useEffect(() => () => {
    dropAnimationsRef.current.forEach(animation => animation.cancel());
    dropAnimationsRef.current.clear();
  }, []);

  const firstVisibleWorkspaceId = visibleReorderableGroups[0]?.workspace.id ?? null;

  useLayoutEffect(() => {
    const pendingReveal = getPendingWorkspaceReveal(surfaceId);
    if (pendingReveal && (!pendingReveal.promoted || firstVisibleWorkspaceId === pendingReveal.workspaceId)) {
      const row = workspaceRowRefs.current.get(pendingReveal.workspaceId);
      if (row) {
        consumeWorkspaceReveal(surfaceId, pendingReveal.token);
        revealWorkspaceRow(row, scrollRootRef?.current);
      }
    }

    const snapshot = pendingDropSnapshotRef.current;
    if (!snapshot) return;
    pendingDropSnapshotRef.current = null;
    if (snapshot.surfaceId !== surfaceId) return;
    if (window.matchMedia?.('(prefers-reduced-motion: reduce)').matches) return;

    const scrollRoot = scrollRootRef?.current;
    const scrollDelta = (scrollRoot?.scrollTop ?? 0) - snapshot.scrollTop;
    const scrollRect = scrollRoot?.getBoundingClientRect();
    const viewportHeight = scrollRect && scrollRect.height > 0 ? scrollRect.height : 0;
    const maxDelta = viewportHeight > 0
      ? Math.max(96, Math.min(WORKSPACE_FLIP_MAX_DELTA_PX, viewportHeight * 0.65))
      : WORKSPACE_FLIP_MAX_DELTA_PX;

    for (const [workspaceId, element] of workspaceRowRefs.current) {
      const previousRect = snapshot.rects.get(workspaceId);
      if (!previousRect) continue;
      const currentRect = element.getBoundingClientRect();
      if (
        scrollRect
        && viewportHeight > 0
        && (
          currentRect.bottom < scrollRect.top - WORKSPACE_FLIP_VIEWPORT_MARGIN_PX
          || currentRect.top > scrollRect.bottom + WORKSPACE_FLIP_VIEWPORT_MARGIN_PX
        )
      ) {
        continue;
      }
      const rawDeltaY = (previousRect.top - currentRect.top) - scrollDelta;
      if (Math.abs(rawDeltaY) < 0.5) continue;
      const deltaY = Math.sign(rawDeltaY) * Math.min(Math.abs(rawDeltaY), maxDelta);

      dropAnimationsRef.current.get(workspaceId)?.cancel();
      const animation = element.animate(
        [
          { transform: `translateY(${deltaY}px)` },
          { transform: 'translateY(0)' },
        ],
        { duration: WORKSPACE_DROP_SETTLE_MS, easing: WORKSPACE_DROP_EASING },
      );
      dropAnimationsRef.current.set(workspaceId, animation);
      animation.addEventListener('finish', () => {
        if (dropAnimationsRef.current.get(workspaceId) === animation) {
          dropAnimationsRef.current.delete(workspaceId);
        }
      }, { once: true });
    }
  }, [firstVisibleWorkspaceId, grouping, revealRevision, scrollRootRef, surfaceId, workspaceSignature]);

  const handleDragStart = useCallback((workspaceId: string) => (event: React.DragEvent<HTMLDivElement>) => {
    const payload: WorkspaceDragPayload = { workspaceId, variant };
    const serializedPayload = JSON.stringify(payload);
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData(WORKSPACE_DRAG_MIME_TYPE, serializedPayload);
    event.dataTransfer.setData('text/plain', serializedPayload);
    resetDragState();
    draggedWorkspaceIdRef.current = workspaceId;
    setDraggedWorkspaceId(workspaceId);
  }, [variant, resetDragState]);

  const handleDragOver = useCallback((workspaceId: string) => (event: React.DragEvent<HTMLDivElement>) => {
    // Browsers block reading dataTransfer data during dragover for security.
    // Check event.dataTransfer.types instead — it IS readable during dragover.
    const isWorkspaceDrag = event.dataTransfer.types.includes(WORKSPACE_DRAG_MIME_TYPE);
    const currentDraggedId = draggedWorkspaceIdRef.current;

    if (!isWorkspaceDrag || !currentDraggedId || currentDraggedId === workspaceId) {
      return;
    }

    event.preventDefault();
    event.stopPropagation();
    event.dataTransfer.dropEffect = 'move';

    // Measure only the workspace card, not the wrapper that includes the drop-line.
    const itemEl = event.currentTarget.querySelector<HTMLElement>(
      '.openbitfun-nav-panel__workspace-item'
    );
    const rect = itemEl
      ? itemEl.getBoundingClientRect()
      : event.currentTarget.getBoundingClientRect();

    const position: WorkspaceDragPosition = event.clientY >= rect.top + rect.height / 2
      ? 'after'
      : 'before';

    setDropTarget(current => {
      if (current?.workspaceId === workspaceId && current.position === position) {
        return current;
      }
      const next = { workspaceId, position };
      dropTargetRef.current = next;
      return next;
    });
  }, []); // Intentionally empty: reads from refs, not closed-over state

  const handleDragLeave = useCallback((workspaceId: string) => (event: React.DragEvent<HTMLDivElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
      setDropTarget(current => {
        if (current?.workspaceId !== workspaceId) return current;
        dropTargetRef.current = null;
        return null;
      });
    }
  }, []);

  const handleDrop = useCallback((workspaceId: string) => async (event: React.DragEvent<HTMLDivElement>) => {
    const sourceWorkspaceId = draggedWorkspaceIdRef.current;
    const position = dropTargetRef.current?.workspaceId === workspaceId
      ? dropTargetRef.current.position
      : 'after';
    // A drop ends the gesture, including malformed or rejected payloads. Saving
    // the order must not keep feedback alive or later clear a newer gesture.
    resetDragState();
    if (!sourceWorkspaceId) return;

    // On drop, reading dataTransfer data IS allowed.
    const payloadText =
      event.dataTransfer.getData(WORKSPACE_DRAG_MIME_TYPE) ||
      event.dataTransfer.getData('text/plain');

    if (!payloadText) return;

    let payload: WorkspaceDragPayload | null;
    try {
      payload = JSON.parse(payloadText) as WorkspaceDragPayload | null;
    } catch {
      return;
    }

    if (!payload || payload.workspaceId !== sourceWorkspaceId || payload.variant !== variant
      || sourceWorkspaceId === workspaceId) return;

    event.preventDefault();
    event.stopPropagation();

    // Keep the slot measured by dragover, before the drop line changed layout.
    pendingDropSnapshotRef.current = captureWorkspaceRowsSnapshot();

    try {
      await reorderOpenedWorkspacesInSection(reorderSection, payload.workspaceId, workspaceId, position);
    } catch (error) {
      pendingDropSnapshotRef.current = null;
      notificationService.error(
        error instanceof Error ? error.message : t('nav.workspaces.reorderFailed'),
        { duration: 4000 }
      );
    }
  }, [captureWorkspaceRowsSnapshot, reorderOpenedWorkspacesInSection, reorderSection, resetDragState, t, variant]);

  /**
   * One session-group row. Pinned assistants and project workspaces share the
   * same box, but only project rows join the drag group: an assistant row never
   * becomes a drop target, so dropping a project near it cannot reorder it.
   */
  const renderGroupRow = (group: WorkspaceBackedSessionGroup, pinned: boolean) => {
    const workspace = group.workspace;
    const isActive = isWorkspaceBackedSessionGroupActive(workspace, activeWorkspace);
    const isSelected = isWorkspaceBackedSessionGroupSelected(workspace, {
      activeTab, contentScope, surfaceId, activeWorkspaceId,
    });
    const draggable = !pinned && canReorderGroups;
    return (
      <div
        data-openbitfun-component="workspace-list-section"
        data-openbitfun-part="item"
        data-openbitfun-state={isSelected ? 'selected' : undefined}
        key={workspace.id}
        ref={pinned ? undefined : (element) => {
          if (element) workspaceRowRefs.current.set(workspace.id, element);
          else workspaceRowRefs.current.delete(workspace.id);
        }}
        className={[
          'openbitfun-nav-panel__workspace-drop-target',
          draggable && draggedWorkspaceId && draggedWorkspaceId !== workspace.id && 'is-drag-active',
          dropTarget?.workspaceId === workspace.id && 'is-drop-target',
          dropTarget?.workspaceId === workspace.id && dropTarget.position === 'before' && 'is-before',
          dropTarget?.workspaceId === workspace.id && dropTarget.position === 'after' && 'is-after',
        ].filter(Boolean).join(' ')}
        data-testid={pinned ? 'nav-workspace-pinned-item' : 'nav-workspace-drop-target'}
        data-workspace-id={workspace.id}
        data-workspace-list={variant}
        data-session-group-kind={group.kind}
        data-session-group-pinned={pinned ? 'true' : undefined}
        onDragOver={draggable ? handleDragOver(workspace.id) : undefined}
        onDragLeave={draggable ? handleDragLeave(workspace.id) : undefined}
        onDrop={draggable ? (event) => { void handleDrop(workspace.id)(event); } : undefined}
      >
        <ProjectWorkspaceItem
          surfaceId={surfaceId}
          isProject={group.kind === 'project'}
          workspace={workspace}
          remoteBadge={remoteBadges.get(workspace.id)}
          isActive={isActive}
          isSelected={isSelected}
          isSingle={!draggable}
          draggable={draggable}
          isDragging={draggedWorkspaceId === workspace.id}
          onDragStart={draggable ? handleDragStart(workspace.id) : undefined}
          onDragEnd={draggable ? resetDragState : undefined}
        />
        {dropTarget?.workspaceId === workspace.id ? (
          <div data-openbitfun-component="workspace-list-section" data-openbitfun-part="dropLine" className="openbitfun-nav-panel__workspace-drop-line" aria-hidden="true" />
        ) : null}
      </div>
    );
  };

  return (
    <div data-openbitfun-component="workspace-list-section" data-openbitfun-part="root" data-openbitfun-state={draggedWorkspaceId ? 'dragging' : undefined}
      className={`openbitfun-nav-panel__workspace-list${draggedWorkspaceId ? ' is-dragging' : ''}`}
      data-testid="nav-workspace-list"
      data-workspace-list={variant}
    >
      {workspaces.length === 0 ? (
        <div
          data-openbitfun-component="workspace-list-section"
          data-openbitfun-part="empty"
          className="openbitfun-nav-panel__workspace-list-empty"
          data-testid="nav-workspace-list-empty"
          data-workspace-list={variant}
        >
          {emptyLabel}
        </div>
      ) : (
        <>
          {pinnedGroups.map(group => renderGroupRow(group, true))}
          {(reorderableGroups.length > 0 || workspaceScopes.length > 0) && (
            <SessionViewTransition grouping={grouping} surfaceId={surfaceId} scrollRootRef={scrollRootRef}>
              {visibleReorderableGroups.map(group => renderGroupRow(group, false))}
              {grouping === 'all' && workspaceScopes.length > 0 && (
                <div className="openbitfun-nav-panel__workspace-all-sessions" data-testid="nav-workspace-all-sessions">
                  <SessionsSection
                    workspaceScopes={workspaceScopes}
                    isVisible
                    layout="flat"
                    limitFlatRows={limitFlatRows}
                    useWorkspaceViewPreferences
                  />
                </div>
              )}
            </SessionViewTransition>
          )}
        </>
      )}
    </div>
  );
};

export default WorkspaceListSection;
