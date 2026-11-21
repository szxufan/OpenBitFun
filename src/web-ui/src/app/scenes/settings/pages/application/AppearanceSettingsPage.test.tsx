// @vitest-environment jsdom

import React, { act } from 'react';
import { createRoot } from 'react-dom/client';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import AppearanceSettingsPage from '@/app/scenes/settings/pages/application/AppearanceSettingsPage';
import { beginComposerSubmission, sessionComposerStore } from '@/flow_chat/store/sessionComposerStore';
import type { Session } from '@/flow_chat/types/flow-chat';
import { activateSurface, LOCAL_SURFACE_ID } from '@/infrastructure/peer-device/deviceSurface';
import appearanceStrings from '@/locales/en-US/settings/appearance.json';

const mocks = vi.hoisted(() => ({
  createChatDraft: vi.fn(),
  getFlowChatState: vi.fn(),
  openMainSession: vi.fn(),
  getWorkspaceState: vi.fn(),
  notifyError: vi.fn(),
}));

vi.mock('@/flow_chat/services/FlowChatManager', () => ({
  flowChatManager: { createChatDraft: mocks.createChatDraft, getFlowChatState: mocks.getFlowChatState },
}));
vi.mock('@/flow_chat/services/sessionActivation', () => ({ openMainSession: mocks.openMainSession }));
vi.mock('@/infrastructure/services/business/workspaceManager', () => ({ workspaceManager: { getState: mocks.getWorkspaceState } }));

vi.mock('react-i18next', async (importOriginal) => ({
  ...await importOriginal<typeof import('react-i18next')>(),
  useTranslation: (namespace: string) => ({
    t: (key: string) => `${namespace}:${key}`,
  }),
}));

vi.mock('@/infrastructure/appearance', () => ({
  SYSTEM_APPEARANCE_ID: 'system',
  useAppearance: () => ({
    selectedAppearanceId: 'system',
    appearances: [],
    select: vi.fn(),
    initialized: true,
    status: 'ready',
  }),
}));

vi.mock('@/infrastructure/i18n', () => ({
  useI18n: () => ({
    t: (key: string, values?: { skill?: string }) => {
      if (key === 'skin.create') return appearanceStrings.skin.create;
      if (key === 'skin.createPrompt') return appearanceStrings.skin.createPrompt.replace('{{skill}}', values?.skill ?? '');
      return key;
    },
  }),
  useLanguageSelector: () => ({
    currentLanguage: 'zh-CN',
    supportedLocales: [{ id: 'zh-CN', nativeName: '简体中文' }],
    selectLanguage: vi.fn(),
    isChanging: false,
  }),
}));

vi.mock('@/shared/notification-system', () => ({
  notificationService: { error: mocks.notifyError },
}));

vi.mock('@/infrastructure/font-preference', () => ({
  FontPreferencePanel: () => <div data-testid="appearance-font-section" />,
}));

vi.mock('../../../../../infrastructure/config/components/AppearancePackageConfigSection', () => ({
  AppearancePackageConfigSection: () => <div data-testid="appearance-package-config" />,
}));

const primary = { id: 'design-assistant', rootPath: '/assistants/design', workspaceKind: 'assistant', assistantId: 'design' };
const skinPrompt = appearanceStrings.skin.createPrompt.replace('{{skill}}', 'openbitfun-skin-designer');

function assistantSession(sessionId: string, overrides: Partial<Session> = {}): Session {
  return {
    sessionId, workspaceId: primary.id, sessionKind: 'normal', mode: 'Claw', historyState: 'new',
    status: 'idle', config: {}, dialogTurns: [], createdAt: 1, lastActiveAt: 1, error: null, ...overrides,
  };
}

function showSessions(sessions: Session[], activeSessionId: string | null = null) {
  mocks.getFlowChatState.mockReturnValue({
    sessions: new Map(sessions.map(session => [session.sessionId, session])), activeSessionId,
  });
}

async function mountPage() {
  const container = document.body.appendChild(document.createElement('div'));
  const root = createRoot(container);
  await act(async () => root.render(<AppearanceSettingsPage />));
  return {
    createButton: container.querySelector<HTMLButtonElement>('[data-testid="visual-skin-import"] + button')!,
    close: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

describe('AppearanceSettingsPage', () => {
  beforeEach(() => {
    globalThis.IS_REACT_ACT_ENVIRONMENT = true;
    mocks.getWorkspaceState.mockReturnValue({
      assistantWorkspaces: [{ id: 'default-assistant', workspaceKind: 'assistant' }, primary],
      openedWorkspaces: new Map(),
      primaryAssistantWorkspaceId: primary.id,
    });
    showSessions([]);
    mocks.createChatDraft.mockResolvedValue('skin-session');
  });

  afterEach(() => {
    document.body.replaceChildren();
    sessionComposerStore.setState({ drafts: {} });
    activateSurface(LOCAL_SURFACE_ID);
    vi.resetAllMocks();
  });

  it('keeps themes and fonts together while language belongs to General', () => {
    document.body.innerHTML = renderToStaticMarkup(<AppearanceSettingsPage />);
    expect(document.querySelector('[data-testid="appearance-package-config"]')).not.toBeNull();
    expect(document.querySelector('[data-testid="appearance-font-section"]')).not.toBeNull();
    expect(document.querySelector('[aria-label="sessionPanel.displayMode"]')).not.toBeNull();
    expect(document.querySelector('[data-testid="appearance-language-select"]')).toBeNull();
    expect(document.querySelector('[data-testid="appearance-palette-select"]')).toBeNull();
    expect(document.querySelector('[data-testid="appearance-package-select"]')).toBeNull();
    expect(document.querySelector('.visual-skin-config .openbitfun-config-page-section__description')).toBeNull();
  });

  it.each(['local', 'peer'])('prefills a fresh primary assistant session on the %s surface', async surfaceId => {
    activateSurface(surfaceId);
    mocks.openMainSession.mockImplementation(async (sessionId: string) => {
      expect(sessionComposerStore.getState().getDraft(sessionId, surfaceId).value).toBe(skinPrompt);
    });

    const page = await mountPage();
    try {
      expect(page.createButton.textContent).toBe(appearanceStrings.skin.create);
      await act(async () => {
        page.createButton.click();
        page.createButton.click();
      });
      expect(mocks.createChatDraft).toHaveBeenCalledExactlyOnceWith({
        workspaceId: primary.id, workspacePath: primary.rootPath,
      }, 'Claw');
      expect(mocks.openMainSession).toHaveBeenCalledExactlyOnceWith('skin-session', {
        workspaceId: primary.id, isCurrent: expect.any(Function),
      });
      expect(mocks.notifyError).not.toHaveBeenCalled();
      if (surfaceId === 'peer') {
        expect(sessionComposerStore.getState().getDraft('skin-session', LOCAL_SURFACE_ID).value).toBe('');
      }
    } finally {
      await page.close();
    }
  });

  it('reuses a blank assistant session while preserving other composer drafts', async () => {
    const composer = sessionComposerStore.getState();
    composer.setValue('unrelated-text', 'My draft');
    composer.setContexts('attached-file', [{ id: 'file', type: 'file', fileName: 'ref.png', filePath: '/ref.png' }]);
    composer.setPendingLargePastes('large-paste', { paste: 'My notes' });
    const finishSubmission = beginComposerSubmission('sending', LOCAL_SURFACE_ID);
    showSessions([
      assistantSession('unrelated-text', { createdAt: 10 }),
      assistantSession('attached-file', { createdAt: 9 }),
      assistantSession('large-paste', { createdAt: 8 }),
      assistantSession('sending', { createdAt: 7 }),
      assistantSession('metadata-only', { createdAt: 6, historyState: 'metadata-only' }),
      assistantSession('different-assistant', { createdAt: 5, workspaceId: 'other' }),
      assistantSession('blank'),
    ], 'unrelated-text');

    const page = await mountPage();
    try {
      await act(async () => page.createButton.click());
      expect(mocks.createChatDraft).not.toHaveBeenCalled();
      expect(mocks.openMainSession).toHaveBeenCalledExactlyOnceWith('blank', expect.objectContaining({ workspaceId: primary.id }));
      expect(composer.getDraft('blank').value).toBe(skinPrompt);
      expect(composer.getDraft('unrelated-text').value).toBe('My draft');
      expect(composer.getDraft('attached-file').contexts).toHaveLength(1);
      expect(composer.getDraft('large-paste').pendingLargePastes).toEqual({ paste: 'My notes' });
    } finally {
      finishSubmission();
      await page.close();
    }
  });

  it('returns to the same edited skin draft after reopening settings until it is sent', async () => {
    const skinSession = assistantSession('skin-session');
    const anotherBlankSession = assistantSession('newer-blank', { createdAt: 2 });
    const composer = sessionComposerStore.getState();
    const firstPage = await mountPage();
    try {
      await act(async () => firstPage.createButton.click());
    } finally {
      await firstPage.close();
    }
    composer.setValue(skinSession.sessionId, 'Design a forest skin');
    composer.setContexts(skinSession.sessionId, [{ id: 'file', type: 'file', fileName: 'forest.png', filePath: '/forest.png' }]);
    showSessions([skinSession, anotherBlankSession], anotherBlankSession.sessionId);

    const page = await mountPage();
    try {
      await act(async () => page.createButton.click());
      expect(mocks.createChatDraft).toHaveBeenCalledTimes(1);
      expect(mocks.openMainSession.mock.calls.map(([id]) => id)).toEqual(['skin-session', 'skin-session']);
      expect(composer.getDraft(skinSession.sessionId)).toMatchObject({
        value: 'Design a forest skin', contexts: [{ id: 'file', fileName: 'forest.png' }],
      });

      showSessions([{ ...skinSession, lastSubmittedMode: 'Claw' }, anotherBlankSession]);
      await act(async () => page.createButton.click());
      expect(mocks.openMainSession).toHaveBeenLastCalledWith('newer-blank', expect.anything());
      expect(mocks.createChatDraft).toHaveBeenCalledTimes(1);
      expect(composer.getDraft(anotherBlankSession.sessionId).value).toBe(skinPrompt);
    } finally {
      await page.close();
    }
  });
});
