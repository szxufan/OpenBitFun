import React from 'react';
import { createRoot } from 'react-dom/client';
import { I18nProvider, i18nService } from '../../src/infrastructure/i18n';
import { i18nAPI } from '../../src/infrastructure/api/service-api/I18nAPI';
import {
  appearanceRuntime,
  buildBuiltinAppearance,
  openBitFunDarkPalette,
} from '../../src/infrastructure/appearance';
import { GenerativeWidgetInline } from '../../src/flow_chat/components/GenerativeWidgetInline';
import { flowChatStore } from '../../src/flow_chat/store/FlowChatStore';
import type { FlowToolItem, Session } from '../../src/flow_chat/types/flow-chat';
import examples from '../../../shared/generative-ui/examples.json';
import '@openbitfun/ui/styles.css';

i18nAPI.getCurrentLanguage = async () => 'en-US';
await i18nService.initialize();
await appearanceRuntime.initialize(buildBuiltinAppearance(openBitFunDarkPalette));
const params = new URLSearchParams(location.search);
let content: React.ReactNode;
if (params.has('streaming')) {
  const Streaming = (await import('./genuiStreaming')).default;
  content = <Streaming deferred={params.has('deferred')} harmony={false} />;
} else {
  const archived = [
    examples.ledger,
    {
      ...examples.picker,
      id: 'saved-openui',
      template: 'openui.app',
      libraryId: 'openui.v1',
      libraryRevision: 1,
      designSpec: { id: 'openui.desktop.v1', revision: 1 },
      source: {
        kind: 'openui-lang',
        content: [
          'root = Col([Text("Stored OpenUI text"), Button("Archived action"), photo, note])',
          'photo = SearchImage("Archived lighthouse")',
          'note = HtmlView("<p>Saved nested note</p><script>document.title = 1</script>", 100, "Saved note")',
        ].join('\n'),
      },
    },
    {
      ...examples.picker,
      id: 'saved-html',
      source: { kind: 'html-document', html: '<article><h2>Stored HTML note</h2></article>' },
    },
    {
      ...examples.picker,
      id: 'historic-phone',
      title: 'Historic phone',
      libraryId: 'openbitfun.harmony.v1',
      libraryRevision: 3,
      source: { kind: 'openui-lang', content: 'root = Col([Text("Archived phone content")])' },
    },
    { schemaVersion: 9, id: 'future', title: 'Future format', storedText: 'Preserved unknown content' },
  ];
  const items: FlowToolItem[] = JSON.parse(
    JSON.stringify(
      archived.map((app, i) => {
        const payload = { app, app_state: i === 0 ? { total: 47 } : {} },
          deferred = i % 2 !== 0;
        return {
          id: 'old-' + i,
          type: 'tool',
          toolName: deferred ? 'CallDeferredTool' : 'GenerativeUI',
          timestamp: i,
          status: 'completed',
          toolCall: {
            id: 'old-call-' + i,
            input: deferred ? { tool_name: 'GenerativeUI', args: payload } : payload,
          },
          toolResult: { success: true, result: payload },
        };
      }),
    ),
  );
  const session = {
    sessionId: 'archive',
    status: 'active',
    config: {},
    createdAt: 1,
    dialogTurns: [{ id: 'turn', modelRounds: [{ items }] }],
  } as Session;
  flowChatStore.setState(() => ({
    sessions: new Map([[session.sessionId, session]]),
    activeSessionId: session.sessionId,
  }));
  content = items.map((item) => (
    <GenerativeWidgetInline key={item.id} toolItem={item} sessionId={session.sessionId} turnId="turn" />
  ));
}
createRoot(document.getElementById('root')!).render(<I18nProvider>{content}</I18nProvider>);
