import React from 'react';
import { createRoot } from 'react-dom/client';
import { ModelRoundItem } from '../../src/flow_chat/components/modern/ModelRoundItem';
import { FlowChatContext } from '../../src/flow_chat/components/modern/FlowChatContext';
import { flowChatStore } from '../../src/flow_chat/store/FlowChatStore';
import type { FlowTextItem, FlowToolItem, Session } from '../../src/flow_chat/types/flow-chat';
import { I18nProvider, i18nService } from '../../src/infrastructure/i18n';
import { i18nAPI } from '../../src/infrastructure/api/service-api/I18nAPI';
import {
  appearanceRuntime,
  buildBuiltinAppearance,
  openBitFunDarkPalette,
  openBitFunLightPalette,
} from '../../src/infrastructure/appearance';
import '@openbitfun/ui/styles.css';
import architecture from './visualize-architecture.json';
import overview from './visualize-overview.json';

i18nAPI.getCurrentLanguage = async () => 'en-US';
await i18nService.initialize();
await appearanceRuntime.initialize(
  buildBuiltinAppearance(
    new URLSearchParams(location.search).has('light') ? openBitFunLightPalette : openBitFunDarkPalette,
  ),
);

const catalog = new URLSearchParams(location.search).has('catalog');
const sample = (kind: string, columns: string[], rows: (string | number)[][]) => ({
  version: 1,
  kind,
  columns,
  rows,
});
const samples = [
  {
    version: 1,
    kind: 'bar',
    columns: ['Release', 'Latency', 'Service'],
    filterColumn: 'Service',
    rows: [
      ['Before', 140, 'A'],
      ['After', 95, 'A'],
      ['Before', 110, 'B'],
      ['After', 103, 'B'],
    ],
    followUp: 'Explain the selected latency.',
  },
  {
    version: 1,
    kind: 'simulation',
    columns: ['Servers', 'Requests per server'],
    rows: [1, 2, 3, 4, 5, 6].map((n) => [n, 360 / n]),
  },
  ...(catalog
    ? [
        ...['line', 'area', 'pie', 'waterfall', 'sparkline', 'ranking', 'funnel'].map((k) =>
          sample(
            k,
            ['Label', 'Value'],
            [
              ['A', 3],
              ['B', 7],
            ],
          ),
        ),
        sample('histogram', ['Duration'], [[1], [3], [3]]),
        sample(
          'heatmap',
          ['Day', 'Team', 'Value'],
          [
            ['Mon', 'A', 4],
            ['Tue', 'A', 5],
          ],
        ),
        sample(
          'scatter',
          ['Seconds', 'Bytes'],
          [
            [1, 200],
            [2, 300],
          ],
        ),
        sample('box-plot', ['Group', 'Min', 'Q1', 'Median', 'Q3', 'Max'], [['A', 1, 3, 5, 9, 10]]),
        sample(
          'sankey',
          ['From', 'To', 'Weight'],
          [
            ['A', 'B', 3],
            ['A', 'C', 2],
          ],
        ),
        sample('calendar', ['Start', 'End', 'Event'], [['2026-11-18T09:00', '2026-11-18T10:00', 'Standup']]),
        sample(
          'request-waterfall',
          ['Request', 'Start', 'End'],
          [
            ['GET /a', 0, 15],
            ['GET /b', 9, 30],
          ],
        ),
        sample(
          'flame-chart',
          ['Stack', 'Samples'],
          [
            ['main;render', 3],
            ['main;commit', 2],
          ],
        ),
        sample(
          'stack-trace',
          ['Function', 'File', 'Line'],
          [
            ['render', 'render.ts', 42],
            ['main', 'main.ts', 12],
          ],
        ),
      ]
    : []),
];
const widgets = [
  {
    kind: 'calendar',
    version: 2,
    date: '2026-11-18',
    timeZone: 'UTC+8',
    events: [
      { title: 'Standup', start: '09:00', end: '09:30', detail: 'Release team' },
      { title: 'Review', start: '13:00', end: '14:00' },
    ],
  },
  {
    kind: 'request-waterfall',
    version: 2,
    criticalPath: ['request', 'sql'],
    spans: [
      { id: 'request', name: 'Request', startMs: 0, durationMs: 120, attributes: { route: 'orders' } },
      { id: 'sql', parentId: 'request', name: 'Database', startMs: 10, durationMs: 80 },
      { id: 'cache', parentId: 'request', name: 'Cache', startMs: 20, durationMs: 15 },
    ],
  },
  {
    kind: 'flame-chart',
    version: 2,
    threadName: 'Main',
    frames: [
      { id: 'main', name: 'render' },
      { id: 'layout', name: 'layout' },
    ],
    samples: [
      { stack: ['main', 'layout'], durationMs: 3 },
      { stack: [], durationMs: 1 },
      { stack: ['main'], durationMs: 2 },
    ],
  },
  {
    kind: 'stack-trace',
    version: 2,
    error: { name: 'Error', message: 'Failed' },
    frames: [
      {
        id: 'app',
        name: 'load',
        line: 8,
        file: 'app.ts',
        source: { startLine: 7, lines: ['read()', 'fail()'] },
      },
      { id: 'native', name: 'dispatch', kind: 'runtime', async: true },
    ],
    causes: [{ name: 'Timeout', message: 'unreachable', stack: 'raw traceback line', frames: [] }],
  },
  {
    kind: 'tabs',
    version: 2,
    panels: [
      {
        title: 'Overview',
        nodes: [
          { title: 'Sample', drawing: [{ d: 'M10 70L70 20L136 70Z', tone: 'face' }] },
          { title: 'Peak', drawing: [{ d: 'M10 70Q73 5 136 70', tone: 'accent' }] },
        ],
        progress: { label: 'Done', value: 6, max: 10 },
        rows: [['OK', 6]],
        columns: ['State', 'Value'],
      },
      {
        title: 'Explore',
        fields: [
          { label: 'Service', options: ['API', 'Worker'] },
          { type: 'checkbox', label: 'Include retries' },
          { type: 'text', label: 'Note' },
          { type: 'range', label: 'Budget', value: 5, min: 0, max: 10 },
        ],
        followUp: 'Explain this service',
      },
    ],
  },
];
const comparison = {
  filterColumn: 'Region',
  charts: [
    {
      id: 'trend',
      title: 'Trend',
      rows: [
        { month: 'March', value: 4, channel: 'A', Region: 'East' },
        { month: 'March', value: 9, channel: 'B', Region: 'West' },
      ],
      spec: { type: 'stackedBar100', x: 'month', y: 'value', series: 'channel' },
    },
    {
      id: 'ranking',
      title: 'Ranking',
      rows: Array.from({ length: 12 }, (_, i) => ({
        name: 'Service ' + i,
        value: i,
        Region: i % 2 ? 'West' : 'East',
      })),
      spec: { type: 'rankedList', category: 'name', value: 'value' },
    },
  ],
};
const params = new URLSearchParams(location.search);
const codex = params.has('codex');
const localPreviews = import.meta.glob<string>('../../../../.visualize-runtime/preview-*.html', { query: '?raw', import: 'default' });
const previewName = params.get('example');
const exampleKey = codex && previewName ? '../../../../.visualize-runtime/preview-' + previewName : undefined;
const example = exampleKey
  ? await localPreviews[exampleKey + '.html']?.()
  : undefined;
const savedReplies = import.meta.glob<{ uri: string; content: string; title?: string }>(
  '../../../../.visualize-runtime/preview-*.json', { import: 'default' },
);
const savedReply = example && exampleKey
  ? await savedReplies[exampleKey + '.json']?.()
  : undefined;
const preview = params.has('overview') ? overview : params.has('architecture') ? architecture : undefined;
const cases = codex ? [{ slug: 'interactive', spec: null }] : preview
  ? [{ slug: params.has('overview') ? 'overview' : 'project-architecture', spec: preview.spec }]
  : new URLSearchParams(location.search).has('html-controls')
  ? [{ slug: 'layers', spec: null }]
  : new URLSearchParams(location.search).has('widgets')
  ? [...widgets.map((spec) => ({ slug: spec.kind, spec })), { slug: 'comparison', spec: comparison }]
  : samples.map((spec) => ({ slug: spec.kind, spec }));
const items: FlowToolItem[] = cases.map(({ slug, spec }, i) => {
  const uri = savedReply?.uri ?? `openbitfun://current-session/artifacts/visualizations/${codex ? 'v2/' : ''}${slug}.html`;
  const source = codex ? (example ?? sessionStorage.getItem('visualize-test-fragment') ?? '<div id="kit-test"><h2>Frequency</h2><input type="range" min="1" max="10" value="2"><output>2 Hz</output></div><script>const r=document.querySelector("input"),o=document.querySelector("output");r.oninput=()=>o.textContent=r.value+" Hz";</script>') : spec === null ? `<figure data-openbitfun-visualize>
    <figcaption>Project layers</figcaption>
    <style>#details { display: none } #services:checked ~ #details { display: block }</style>
    <input type="radio" name="layer" id="services"><label for="services">Services</label>
    <p id="details">Session storage</p>
  </figure>` : `<figure data-openbitfun-visualize><figcaption>${preview?.title ?? slug}</figcaption><script type="application/json">${JSON.stringify(spec)}</script></figure>`;
  const args = { payload: '+++ ' + uri + '\n' + source };
  return {
    id: 'write-' + i,
    type: 'tool',
    toolName: i ? 'CallDeferredTool' : 'Write',
    timestamp: i,
    status: 'completed',
    toolCall: { id: 'call-' + i, input: i ? { tool_name: 'Write', args } : args },
    toolResult: {
      success: true,
      result: { file_path: uri, bytes_written: new TextEncoder().encode(source).length },
    },
  };
});
const message: FlowTextItem = {
  id: 'answer',
  type: 'text',
  isMarkdown: true,
  isStreaming: false,
  status: 'completed',
  timestamp: 3,
  content: savedReply?.content ?? (params.has('plain-reply') ? 'Explore the selected item.' : items
    .map((item, i) => {
      if (codex) return 'visualize' + JSON.stringify({ path: item.toolResult!.result.file_path, title: 'Interactive preview', mode: 'wide' }) + '';
      if (preview) return `[${preview.title}](${item.toolResult!.result.file_path})`;
      const summary = i === 0 ? 'Compare latency' : i === 1 ? 'Explore load' : cases[i].slug;
      return 'Report ' + (i + 1) + '\n\n[' + summary + '](' + item.toolResult!.result.file_path + ')';
    })
    .join('\n\n')),
};
const session = {
  sessionId: 'visual-test',
  status: 'active',
  config: {},
  createdAt: 1,
  dialogTurns: [
    {
      id: 'turn',
      sessionId: 'visual-test',
      userMessage: { id: 'user', content: params.get('deferred-request') ?? 'Compare latency and load', timestamp: 0 },
      ...(params.has('deferred-request') && { publishedFigures: Object.fromEntries(items.map(item => {
        const uri = item.toolResult!.result.file_path;
        const source = sessionStorage.getItem('visualize-test-fragment')!;
        return [uri, { source, contentBytes: new TextEncoder().encode(source).length, inspectionDeferred: true }];
      })) }),
      status: 'completed',
      startTime: 0,
      modelRounds: [
        {
          id: 'round',
          index: 0,
          items: [...items, message],
          isStreaming: false,
          isComplete: true,
          status: 'completed',
          startTime: 0,
        },
      ],
    },
  ],
} as Session;
flowChatStore.setState(() => ({
  sessions: new Map([[session.sessionId, session]]),
  activeSessionId: session.sessionId,
}));
// Exercise late tool completion through the same store to which the mounted figure subscribes.
window.addEventListener('fixture:csv', event => {
  const status = (event as CustomEvent<'running' | 'completed'>).detail;
  const html = sessionStorage.getItem('visualize-test-fragment')!;
  const uri = items[0].toolResult!.result.file_path;
  const path = uri.replace(/[^/]+$/, 'cities.csv');
  const csv = 'Shanghai,3';
  const file: FlowToolItem = { ...items[0], id: 'csv', status,
    toolCall: { id: 'csv', input: { payload: '+++ ' + path + '\n' + csv } },
    toolResult: status === 'completed' ? { success: true, result: { file_path: path, content_bytes: csv.length } } : undefined };
  flowChatStore.setState(state => {
    const current = state.sessions.get(session.sessionId)!;
    const turn = current.dialogTurns[0], round = turn.modelRounds[0];
    const next = { ...turn, modelRounds: [{ ...round, items: [file, ...items, message] }],
      publishedFigures: turn.publishedFigures ?? { [uri]: { source: html, contentBytes: new TextEncoder().encode(html).length, bindings: {} } } };
    return { sessions: new Map(state.sessions).set(current.sessionId, { ...current, dialogTurns: [next] }) };
  });
});
const history = new URLSearchParams(location.search).has('history');
let archive: React.ReactNode;
if (history) {
  const { GenerativeWidgetInline } = await import('../../src/flow_chat/components/GenerativeWidgetInline');
  const archived = (await import('../../../shared/generative-ui/examples.json')).default.picker;
  const historyItems: FlowToolItem[] = JSON.parse(
    JSON.stringify(
      [false, true].map((deferred, i) => {
        const app = {
          ...archived,
          id: 'archive-' + i,
          title: 'Saved ' + i,
          source: {
            kind: 'openui-lang',
            content: 'root = Col([Text("Saved record ' + i + '"), Button("Archived action")])',
          },
        };
        return {
          id: 'legacy-' + i,
          type: 'tool',
          toolName: deferred ? 'CallDeferredTool' : 'GenerativeUI',
          status: 'completed',
          timestamp: 1,
          toolCall: {
            id: 'legacy-call-' + i,
            input: deferred ? { tool_name: 'GenerativeUI', args: { app } } : { app },
          },
          toolResult: { success: true, result: { app, app_state: {} } },
        };
      }),
    ),
  );
  session.dialogTurns[0].modelRounds[0].items = historyItems;
  archive = historyItems.map((item) => (
    <GenerativeWidgetInline key={item.id} toolItem={item} sessionId={session.sessionId} turnId="turn" />
  ));
}
const root = document.getElementById('root')!;
if (preview || example) {
  document.documentElement.lang = 'zh-CN';
  document.title = preview?.title ?? savedReply?.title ?? (previewName === 'session' ? 'SparkLab' : 'OpenBitFun');
  document.body.style.backgroundColor = 'var(--openbitfun-color-surface-canvas)';
  root.style.cssText = 'max-width:48rem;margin:clamp(1rem,6vw,4rem) auto;padding:0 1rem';
}
createRoot(root).render(
  <I18nProvider>
    <FlowChatContext.Provider value={{ sessionId: session.sessionId }}>
      {history ? archive : session.dialogTurns[0].modelRounds.map(round => (
        <ModelRoundItem key={round.id} round={round} turnId={session.dialogTurns[0].id} blockPart="content" />
      ))}
    </FlowChatContext.Provider>
  </I18nProvider>,
);
