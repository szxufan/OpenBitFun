import '@/app/scenes/settings/pages/application/AppearanceSettingsPage.scss';
import { Button } from '@openbitfun/ui';
import { Paintbrush } from 'lucide-react';
import { useRef, useState } from 'react';
import { flowChatSessionConfigForWorkspace, pickPrimaryAssistantWorkspace } from '@/app/utils/projectSessionWorkspace';
import { flowChatManager } from '@/flow_chat/services/FlowChatManager';
import { pendingQueueManager } from '@/flow_chat/services/flow-chat-manager/PendingQueueModule';
import { openMainSession } from '@/flow_chat/services/sessionActivation';
import { SessionExecutionState, stateMachineManager } from '@/flow_chat/state-machine';
import { isComposerSubmissionPending, sessionComposerStore } from '@/flow_chat/store/sessionComposerStore';
import { canRestoreQueuedMessageToComposer } from '@/flow_chat/utils/pendingQueueDraft';
import { isSessionBindingLocked } from '@/flow_chat/utils/sessionLifecycle';
import { compareSessionsForNavStable, sessionBelongsToWorkspaceNavRow } from '@/flow_chat/utils/sessionOrdering';
import { AppearancePackageConfigSection } from '@/infrastructure/config/components/AppearancePackageConfigSection';
import { VisualSkinConfigSection } from '@/infrastructure/config/components/VisualSkinConfigSection';
import {
  ConfigPageContent,
  ConfigPageHeader,
  ConfigPageLayout,
  ConfigPageSectionStack
} from '@/infrastructure/config/components/common';
import { FontPreferencePanel } from '@/infrastructure/font-preference';
import { useI18n } from '@/infrastructure/i18n';
import { getActiveSurfaceScope, isSurfaceChangedError, type DeviceSurfaceId } from '@/infrastructure/peer-device/deviceSurface';
import { workspaceManager } from '@/infrastructure/services/business/workspaceManager';
import { notificationService } from '@/shared/notification-system';
import SessionPanelSettingsSection from './SessionPanelSettingsSection';
import FlowChatWidthSettingsSection from './FlowChatWidthSettingsSection';
import SidebarSettingsSection from './SidebarSettingsSection';

function findSkinDesignSession(workspaceId: string, surfaceId: DeviceSurfaceId) {
  const { activeSessionId, sessions } = flowChatManager.getFlowChatState();
  const composer = sessionComposerStore.getState();
  const candidates = Array.from(sessions.values()).filter(session => (
    sessionBelongsToWorkspaceNavRow(session, workspaceId)
    && session.mode?.toLowerCase() === 'claw'
    && session.sessionKind === 'normal' && !session.parentSessionId && !session.isTransient
    && !session.externalHistory && !session.config.dispatchTarget && !session.config.dispatchJobId
    && session.status !== 'error' && session.persistedStatus !== 'archived' && session.persistedStatus !== 'completed'
    && !session.isPartial && !session.persistedTurnCount && !isSessionBindingLocked(session)
    && (!session.draft || session.draft.phase === 'editing')
  )).sort(compareSessionsForNavStable);

  let emptySessionId: string | undefined;
  for (const { sessionId } of candidates) {
    const machine = stateMachineManager.getSnapshot(sessionId);
    if (isComposerSubmissionPending(sessionId, surfaceId)
      || (machine && machine.currentState !== SessionExecutionState.IDLE)
      || machine?.context.queuedInput || pendingQueueManager.listForSurface(surfaceId, sessionId).length) continue;
    const draft = composer.getDraft(sessionId, surfaceId);
    if (draft.prefillSource === 'skin-design') return { sessionId, needsPrefill: false };
    if (canRestoreQueuedMessageToComposer(draft) && (!emptySessionId || sessionId === activeSessionId)) {
      emptySessionId = sessionId;
    }
  }
  return emptySessionId ? { sessionId: emptySessionId, needsPrefill: true } : undefined;
}

function AppearanceSettingsPage() {
  const { t } = useI18n('settings/appearance');
  const creationPending = useRef(false);
  const [creatingSkin, setCreatingSkin] = useState(false);

  async function handleCreateSkin() {
    if (creationPending.current) return;
    const scope = getActiveSurfaceScope();
    const { assistantWorkspaces, openedWorkspaces, primaryAssistantWorkspaceId } = workspaceManager.getState();
    const workspace = pickPrimaryAssistantWorkspace(
      assistantWorkspaces ?? Array.from(openedWorkspaces.values()),
      primaryAssistantWorkspaceId,
    );
    if (!workspace) {
      notificationService.error(t('skin.assistantUnavailable'));
      return;
    }

    creationPending.current = true;
    setCreatingSkin(true);
    try {
      const existing = findSkinDesignSession(workspace.id, scope.surfaceId);
      const sessionId = existing?.sessionId
        ?? await flowChatManager.createChatDraft(flowChatSessionConfigForWorkspace(workspace), 'Claw');
      if (!scope.isCurrent()) return;
      if (existing?.needsPrefill !== false) {
        sessionComposerStore.getState().setPrefill(
          sessionId,
          t('skin.createPrompt', { skill: 'openbitfun-skin-designer' }),
          'skin-design',
          scope.surfaceId,
        );
      }
      await openMainSession(sessionId, { workspaceId: workspace.id, isCurrent: scope.isCurrent });
    } catch (error) {
      if (scope.isCurrent() && !isSurfaceChangedError(error)) {
        notificationService.error(error instanceof Error ? error.message : t('skin.createFailed'));
      }
    } finally {
      creationPending.current = false;
      setCreatingSkin(false);
    }
  }

  return (
    <ConfigPageLayout
      className="openbitfun-appearance-settings"
      data-openbitfun-component="appearance-settings"
      data-openbitfun-part="root"
    >
      <ConfigPageHeader title={t('title')} />
      <ConfigPageContent
        className="openbitfun-appearance-settings__content"
        data-openbitfun-component="appearance-settings"
        data-openbitfun-part="content"
      >
        <ConfigPageSectionStack data-testid="appearance-settings">
          <AppearancePackageConfigSection />
          <VisualSkinConfigSection createAction={(
            <Button
              variant="outline"
              size="sm"
              leadingIcon={<Paintbrush size={14} aria-hidden="true" />}
              loading={creatingSkin}
              disabled={creatingSkin}
              onClick={() => void handleCreateSkin()}
            >
              {t('skin.create')}
            </Button>
          )} />
          <FontPreferencePanel />
          <SidebarSettingsSection />
          <FlowChatWidthSettingsSection />
          <SessionPanelSettingsSection />
        </ConfigPageSectionStack>
      </ConfigPageContent>
    </ConfigPageLayout>
  );
}

export default AppearanceSettingsPage;
