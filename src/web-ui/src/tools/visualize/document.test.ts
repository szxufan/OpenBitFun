// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import type { FlowTextItem, FlowToolItem, Session } from '@/flow_chat/types/flow-chat';
import { buildFlowItemGroups } from '@/flow_chat/grouping/roundGroups';
import { isToolCardVisible } from '@/flow_chat/utils/flowItemVisibility';
import { visualReplyParts } from './reference';
import { filterVisualTable, parseVisualDocument } from './document';
import { findVisualWrite, missingInlineFigures, readVisualWrite, readVisualRevision } from './writeSnapshot';

function markup(value: unknown) {
  return (
    '<figure data-openbitfun-visualize><figcaption>Samples</figcaption><script type="application/json">' +
    JSON.stringify(value) +
    '</script></figure>'
  );
}

describe('visual answer contract', () => {
  it('preserves missing and zero values and hides only the filter dimension', () => {
    const parsed = parseVisualDocument(
      markup({
        version: 1,
        kind: 'line',
        columns: ['sample', 'n', 'group'],
        filterColumn: 'group',
        rows: [
          ['day 1', 0, 'a'],
          ['day 2', null, 'a'],
          ['day 1', 42, 'b'],
        ],
      }),
    );
    expect(parsed.kind).toBe('visual');
    if (parsed.kind !== 'visual') throw Error('Missing document');
    expect(filterVisualTable(parsed.spec, 'a')).toEqual({
      columns: ['sample', 'n'],
      rows: [
        ['day 1', 0],
        ['day 2', null],
      ],
    });
  });
  it.each([
    { version: 2, kind: 'tabs', panels: [{ title: 'Run', variants: [{ label: 'A' }, { label: 'B' }] }] },
    { version: 2, kind: 'table', columns: ['name'], rows: [['a']] },
    { version: 1, kind: 'line', columns: ['x', 'y'], rows: [[1]] },
    {
      version: 1,
      kind: 'sankey',
      columns: ['from', 'to', 'value'],
      rows: [
        ['a', 'b', 1],
        ['b', 'a', 1],
      ],
    },
    {
      version: 1,
      kind: 'box-plot',
      columns: ['x', 'min', 'q1', 'median', 'q3', 'max'],
      rows: [['a', 0, 2, 1, 3, 4]],
    },
    { version: 1, kind: 'line', columns: ['x', 'y'], rows: [[1, 42]], formatter: 'arbitrary script' },
  ])('rejects unavailable contracts and invalid data', (spec) => {
    expect(parseVisualDocument(markup(spec))).toEqual({ kind: 'invalid' });
  });
  it('keeps unrelated HTML in the static sandbox path', () => {
    expect(parseVisualDocument('<script>draw()</script>')).toEqual({ kind: 'invalid' });
    expect(parseVisualDocument('<h2>A</h2>')).toEqual({ kind: 'static', html: '<h2>A</h2>', hasControls: false });
  });
  it('replays marked HTML figures with native controls and no chart specification', () => {
    const html = '<figure data-openbitfun-visualize><figcaption>Layers</figcaption><details><summary>Services</summary>Storage</details></figure>';
    expect(parseVisualDocument(html)).toEqual({ kind: 'static', html, hasControls: true });
    expect(parseVisualDocument(html + '<script>draw()</script>')).toEqual({ kind: 'invalid' });
    expect(parseVisualDocument(html.replace('</figure>', '<script type="application/json">{</script></figure>'))).toEqual({ kind: 'invalid' });
  });
});

const uri = 'openbitfun://current-session/artifacts/visualizations/report.html';
function write(id: string, source: string, deferred = false): FlowToolItem {
  const args = { payload: '+++ ' + uri + '\n' + source };
  return {
    id,
    type: 'tool',
    timestamp: 1,
    status: 'completed',
    toolName: deferred ? 'CallDeferredTool' : 'Write',
    toolCall: { id, input: deferred ? { tool_name: 'Write', args } : args },
    toolResult: {
      success: true,
      result: { file_path: uri, bytes_written: new TextEncoder().encode(source).length },
    },
  };
}
function history(...turns: (FlowTextItem | FlowToolItem)[][]): Session {
  return { dialogTurns: turns.map((items, i) => ({ id: String(i), modelRounds: [{ items }] })) } as Session;
}
const message = { id: 'reply', type: 'text' } as FlowTextItem;
describe('session-scoped visual sources', () => {
  it.each([false, true])('renders the reply without a file row and still resolves its saved Write (deferred=%s)', deferred => {
    const item = write('source', '<h2>Architecture</h2>', deferred);
    const reply = { ...message, content: '[Architecture](' + uri + ')' };
    const saved = JSON.parse(JSON.stringify(history([item, reply]))) as Session;
    const round = saved.dialogTurns[0].modelRounds[0];
    expect(buildFlowItemGroups({ items: round.items })).toEqual([{ type: 'critical', item: reply }]);
    expect(readVisualWrite(findVisualWrite(saved, reply.id, uri), uri)).toBe('<h2>Architecture</h2>');
  });

  it('waits for the streamed destination and keeps ordinary writes and actionable states visible', () => {
    const base = { ...write('source', ''), toolResult: undefined, status: 'receiving' as const, isParamsStreaming: true };
    const streaming = (payload: string) => ({ ...base, toolCall: { id: base.id, input: { payload } } });
    for (const payload of ['', '+++ open', '+++ ' + uri, '+++ ' + uri + '\n<h2>']) {
      expect(isToolCardVisible(streaming(payload))).toBe(false);
    }
    const ordinary = streaming('+++ /project/report.html\n<h2>');
    expect(isToolCardVisible(ordinary)).toBe(true);
    const visual = streaming('+++ ' + uri + '\n<h2>');
    expect(isToolCardVisible({ ...visual, status: 'pending_confirmation' })).toBe(true);
    expect(isToolCardVisible({ ...visual, status: 'error' })).toBe(true);
    expect(isToolCardVisible({ ...visual, importedSource: 'codex' })).toBe(true);
  });

  it.each([false, true])('keeps background cards absent from the first parameter fragment (deferred=%s)', deferred => {
    const path = 'openbitfun://current-session/artifacts/visualizations/v2/map.html';
    function streaming(name: string, args: Record<string, unknown>): FlowToolItem {
      return { ...write('id', ''), toolName: deferred ? 'CallDeferredTool' : name,
        toolCall: { id: 'id', input: deferred ? { tool_name: name, args } : args },
        status: 'streaming', isParamsStreaming: true, toolResult: undefined };
    }
    for (let end = 0; end <= path.length; end++) {
      expect(isToolCardVisible(streaming('Edit', { file_path: path.slice(0, end) }))).toBe(false);
    }
    for (let end = 0; end <= 'visualize'.length; end++) {
      expect(isToolCardVisible(streaming('Skill', { command: 'visualize'.slice(0, end) }))).toBe(false);
    }
    expect(isToolCardVisible(streaming('Edit', { file_path: 'src/app.js' }))).toBe(true);
    expect(isToolCardVisible(streaming('Skill', { command: 'search' }))).toBe(true);
    expect(isToolCardVisible({ ...streaming('Edit', { file_path: path }), status: 'pending_confirmation' })).toBe(true);
    if (deferred) expect(isToolCardVisible(streaming('Edi', {}))).toBe(false);
  });

  it('reopens the matching successful revision in an old serialized tool turn', () => {
    const oldWrite = write('before', '温度'),
      updated = write('after', 'New', true);
    const session = JSON.parse(JSON.stringify(history([oldWrite, message, updated], [updated])));
    expect(readVisualWrite(findVisualWrite(session, 'reply', uri), uri)).toBe('温度');
  });
  it('handles wrapped writes, idempotent writes and byte-truncated transport data', () => {
    const item = write('retry', 'Δ', true);
    Object.assign(item.toolResult!.result, { bytes_written: 0, content_bytes: 2 });
    expect(readVisualWrite(findVisualWrite(history([item, message]), 'reply', uri), uri)).toBe('Δ');
    item.toolResult!.result.content_bytes++;
    expect(readVisualWrite(item, uri)).toBeUndefined();
  });
  it('does not read a previous turn, future output, a denied write, or an arbitrary path', () => {
    const done = write('write', 'OK');
    const denied = { ...done, status: 'rejected' } as FlowToolItem;
    expect(findVisualWrite(history([done], [denied, message, done]), 'reply', uri)).toBeUndefined();
    expect(
      findVisualWrite(history([{ ...done, subagentSessionId: 'child' }, message]), 'reply', uri),
    ).toBeUndefined();
    for (const path of [
      '/tmp/report.html',
      uri + '/../report.html',
      uri.replace('current-session', 'runtime'),
    ]) {
      expect(findVisualWrite(history([done, message]), 'reply', path)).toBeUndefined();
    }
  });
});

describe('scoped v2 data bindings', () => {
  const figureUri = uri.replace('visualizations/', 'visualizations/v2/');
  const fileUri = figureUri.replace('report.html', 'counts.csv');
  function asset(path: string, content: string, id: string) {
    const item = write(id, content);
    item.toolCall.input.payload = '+++ ' + path + '\n' + content;
    item.toolResult!.result.file_path = path;
    return item;
  }
  it('renders the sealed bytes only in the final reply and rejects incomplete snapshots', () => {
    const file = asset(figureUri, 'draft', 'map');
    const early = { ...message, id: 'early' };
    const session = history([file, early, message]);
    session.dialogTurns[0].status = 'completed';
    const snapshots = { [figureUri]: { source: '地图', contentBytes: 6 } };
    session.dialogTurns[0].publishedFigures = snapshots;
    expect(readVisualRevision(session, file, early.id, figureUri)).toBe('draft');
    expect(readVisualRevision(session, file, message.id, figureUri)).toBe('地图');
    expect(readVisualRevision(session, message, message.id, figureUri)).toBe('地图');
    snapshots[figureUri].contentBytes = 4;
    expect(readVisualRevision(session, file, message.id, figureUri)).toBeUndefined();
    session.dialogTurns[0].publishedFigures = { [figureUri]: null };
    expect(readVisualRevision(session, file, message.id, figureUri)).toBeUndefined();
    delete session.dialogTurns[0].publishedFigures;
    expect(readVisualRevision(session, file, message.id, figureUri)).toBe('draft');
  });
  const tool = { ...write('data', ''), toolName: 'CallDeferredTool', toolCall: { id: 'data', input: { tool_name: 'Counts', args: { b: 2, a: 1 } } }, toolResult: { success: true, result: { count: 9 } } };
  const header = (declarations: unknown) => '<!-- vis-bindings\n' + JSON.stringify(declarations) + '\n-->\n<output/>';
  it('delivers only the matching producer, source and arguments; replay excludes later data', async () => {
    const { readBindings, resolveBindingSnapshots, inspectionBindings } = await import('./visualBindings');
    const source = header({
      rows: { type: 'file', path: './counts.csv' }, catalog: { type: 'library', libraryFileId: 'libfile_sales', refreshIntervalSeconds: 15 },
      counts: { type: 'tool', tool: 'Counts', arguments: { a: 1, b: 2 }, refreshIntervalSeconds: 60 }, baseline: { type: 'json', value: 0 },
    });
    const figure = asset(figureUri, source, 'figure');
    const file = asset(fileUri, 'n\n2', 'csv');
    expect(isToolCardVisible(file)).toBe(false);
    const library = asset('openbitfun://current-session/artifacts/library/libfile_sales', 'latest', 'lib');
    const other = { ...tool, id: 'child', subagentSessionId: 'child', toolResult: { success: true, result: { count: 999 } } };
    const later = { ...tool, toolResult: { success: true, result: { count: 1000 } } };
    const session = history([file, library, tool, other, figure, later]);
    const result = resolveBindingSnapshots(readBindings(source)!, session, figure, figureUri);
    session.sessionId = 'producer';
    const owner = { sessionId: session.sessionId, turnId: session.dialogTurns[0].id, toolCallId: figure.toolCall.id, uri: figureUri };
    expect(inspectionBindings(readBindings(source)!, session, owner)).toEqual(result);
    expect(inspectionBindings(readBindings(source)!, session, { ...owner, sessionId: 'other' })).toBeUndefined();
    expect(inspectionBindings(readBindings(source)!, session, { ...owner, toolCallId: 'unknown' })).toBeUndefined();
    expect(result.counts).toEqual({ kind: 'json', value: { count: 9 } });
    expect(result.rows.value).toEqual({ text: 'n\n2', name: 'counts.csv', mimeType: 'text/csv' });
    expect(result.catalog.value).toMatchObject({ text: 'latest' });
    expect(result.baseline.value).toBe(0);
    expect(resolveBindingSnapshots(readBindings(source)!, history([file, library, tool], [figure]), figure, figureUri)).toEqual({ baseline: { kind: 'json', value: 0 } });
  });
  it('recovers an empty or partial old receipt when the preceding CSV arrives', async () => {
    const { readBindings, inspectionBindings, readerBindingSnapshot, resolveBindingSnapshots } = await import('./visualBindings');
    const source = header({ rows: { type: 'file', path: './counts.csv' }, count: { type: 'json', value: 4 } });
    const declarations = readBindings(source)!;
    const file = asset(fileUri, 'city,count', 'csv');
    const figure = asset(figureUri, source, 'graph');
    const session = history([file, figure, asset(fileUri, 'later revision', 'later')]);
    const owner = { sessionId: session.sessionId, turnId: session.dialogTurns[0].id, toolCallId: figure.toolCall.id, uri: figureUri };
    file.status = 'running';
    expect(inspectionBindings(declarations, session, owner)).toBeUndefined();
    const resolve = () => resolveBindingSnapshots(declarations, session, figure, figureUri);
    const count = { kind: 'json' as const, value: 0 };
    const read = readerBindingSnapshot(declarations, resolve, { count });
    const empty = readerBindingSnapshot(declarations, resolve, {});
    const waiting = read();
    expect(read()).toBe(waiting);
    expect(empty().rows).toBeUndefined();
    file.status = 'completed';
    const complete = read();
    expect(complete.count).toBe(count);
    expect(complete.rows.value).toMatchObject({ text: 'city,count' });
    expect(empty().rows).toEqual(complete.rows);
    expect(read()).toBe(complete);
  });
  it('restores prior edit records and exact owner revisions, up to the referenced reply', () => {
    const original = asset(figureUri, '<b>old</b>', 'figure');
    const input = { file_path: figureUri, old_string: 'old', new_string: 'new' };
    const edit = { ...original, id: 'edit', toolName: 'Edit', toolCall: { id: 'edit', input },
      toolResult: { success: true, result: { ...input, match_count: 1 } } };
    const saved = JSON.parse(JSON.stringify(history([original, edit, message, edit])));
    const first = findVisualWrite(saved, message.id, figureUri);
    expect(readVisualRevision(saved, first, message.id, figureUri)).toBe('<b>new</b>');
    expect(isToolCardVisible(edit)).toBe(false);
    const source = '<b>Δ</b>';
    Object.assign(edit.toolResult.result, { visualization_source: source, content_bytes: new TextEncoder().encode(source).length });
    expect(readVisualRevision(history([original, edit, message]), original, message.id, figureUri)).toBe(source);
    const editedTurn = history([original, edit, message]);
    expect(findVisualWrite(editedTurn, message.id, figureUri)).toBe(original);
    expect(findVisualWrite(editedTurn, message.id, figureUri, undefined, 'revision')).toBe(edit);
    const nextTurn = history([original], [edit, message]);
    expect(readVisualRevision(nextTurn, findVisualWrite(nextTurn, message.id, figureUri), message.id, figureUri)).toBe(source);
    Object.assign(edit.toolResult.result, { content_bytes: 0 });
    expect(readVisualRevision(nextTurn, findVisualWrite(nextTurn, message.id, figureUri), message.id, figureUri)).toBeUndefined();
  });
  it('renders a follow-up from its host-attested native path, including when only the reply page is loaded', () => {
    const path = '/home/example/.openbitfun/projects/p/sessions/s/artifacts/visualizations/v2/clock.html';
    const uri = 'openbitfun://current-session/artifacts/visualizations/v2/clock.html';
    const source = '<canvas aria-label="Clock"></canvas>';
    const edit: FlowToolItem = { ...write('edit', ''), toolName: 'Edit', toolCall: { id: 'edit', input: { file_path: path } },
      toolResult: { success: true, result: { file_path: path, visualization_uri: uri, visualization_source: source, content_bytes: source.length } } };
    const saved = history([edit, message]);
    expect(readVisualRevision(saved, findVisualWrite(saved, message.id, uri), message.id, uri)).toBe(source);
    expect(isToolCardVisible(edit)).toBe(false);
    edit.toolCall.input = { file_path: '/tmp/clock.html' };
    expect(findVisualWrite(saved, message.id, uri)).toBeUndefined();
    delete edit.toolResult!.result.visualization_uri;
    expect(isToolCardVisible(edit)).toBe(true);
  });
  it('recovers figures from completed turns only and honors explicit references', () => {
    const figure = asset(figureUri, '<h2>Diagram</h2>', 'figure');
    const reply: FlowTextItem = { ...message, status: 'completed', isStreaming: false, content: 'Here is the diagram.' };
    const turn = history([figure, { ...figure, id: 'child', subagentSessionId: 'other' }, reply]).dialogTurns[0];
    turn.status = 'completed';
    expect(missingInlineFigures(turn, reply)).toEqual([figureUri]);
    expect(missingInlineFigures({ ...turn, status: 'error' }, reply)).toEqual([]);
    const damaged = '㈀visualize㈁' + JSON.stringify({ path: figureUri }) + '㈂';
    reply.content = 'Before.\n\n' + damaged + '\n\nAfter.';
    expect(missingInlineFigures(turn, reply)).toEqual([figureUri]);
    expect(visualReplyParts(reply.content).join('')).toMatch(/^Before\.\s+After\.$/);
    for (const marker of [damaged, 'visualize' + JSON.stringify({ path: figureUri }) + '']) {
      for (let end = 1; end < marker.length; end++) {
        expect(visualReplyParts(marker.slice(0, end))).toEqual([]);
      }
    }
    expect(visualReplyParts('A ' + figureUri + ' B')).toEqual(['A  B']);
    expect(visualReplyParts('[Map](' + figureUri + ')')).toEqual([{ path: figureUri, title: 'Map', wide: false }]);
    for (const content of ['[Diagram](' + figureUri + ')', 'visualize' + JSON.stringify({ path: figureUri }) + '']) {
      reply.content = content;
      expect(missingInlineFigures(turn, reply)).toEqual([]);
    }
  });
  it('recovers sealed main figures after receipts are compacted, respecting markers, tombstones and child ownership', () => {
    const another = figureUri.replace(/[^/]+$/, 'second.html');
    const rejected = figureUri.replace(/[^/]+$/, 'removed.html');
    const reply: FlowTextItem = { ...message, status: 'completed', isStreaming: false, content: '[Shown](' + another + ')' };
    const child = { ...reply, id: 'child', subagentSessionId: 'child-agent', content: 'Child summary' };
    const turn = history([reply, child]).dialogTurns[0];
    turn.modelRounds.push({ ...turn.modelRounds[0], items: [] });
    turn.status = 'completed';
    turn.publishedFigures = { [figureUri]: { source: 'x', contentBytes: 1 }, [another]: { source: 'y', contentBytes: 1 }, [rejected]: null };
    expect(missingInlineFigures(turn, reply)).toEqual([figureUri]);
    expect(missingInlineFigures(turn, child)).toEqual([]);
  });
  it('rejects entire malformed declarations, credentialed image URLs and traversal', async () => {
    const { readBindings } = await import('./visualBindings');
    expect(readBindings('<!-- vis-bindings\n{"a":{"type":"json","value":1,"value":2}}\n-->\n')).toBeNull();
    for (const bad of [{ type: 'file', path: './../secret' },{ type: 'image', url: 'https://name@example.org/image' },
      {type:'tool', tool:'Counts', arguments:{}, refreshIntervalSeconds:1}, {type:'json',value:'x', extra:2}]) {
      expect(readBindings(header({ good:{ type:'json', value:0 }, bad }))).toBeNull();
    }
  });
});
