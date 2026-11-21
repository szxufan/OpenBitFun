import { lazy, Suspense, useCallback, useMemo, useState, useSyncExternalStore } from 'react';
import { useFlowChatContext } from '@/flow_chat/components/modern/FlowChatContext';
import { flowChatStore } from '@/flow_chat/store/FlowChatStore';
import { getActiveSurfaceScope, onSurfaceActivated } from '@/infrastructure/peer-device/deviceSurface';
import { useI18n } from '@/infrastructure/i18n';
import { useConversationViewScope } from '@/flow_chat/contexts/conversationViewScope';
import {
  filterVisualTable,
  parseVisualDocument,
  parseBoundChart,
  type VisualDocument,
  type VisualSpec,
} from './document';
import { findVisualWrite, publishedFigureReply, readVisualRevision } from './writeSnapshot';
import { VisualCalendar } from './VisualCalendar';
import { VisualStackTrace } from './VisualStackTrace';
import { WidgetView, useWidgetState } from './WidgetView';
import './VisualAnswer.scss';
import { isCodexVisualUri } from './reference';
import type { BoundValues } from './visualBindings';
import { staticFigureTypography } from './figureAppearance';
import inlineDocumentCss from './inlineDocument.css?raw';

const emptyNames: string[] = [];
const VisualChart = lazy(() => import('./VisualChart'));
const StaticDocument = lazy(() => import('@/shared/ui/static-document/StaticHtmlDocument'));
const CodexFigure = lazy(() => import('./CodexFigure'));

interface ReaderState {
  group: string | null;
  row: number;
  hidden?: string[];
  limit?: number;
}
function readPreferences(key: string): ReaderState {
  try {
    const value = JSON.parse(localStorage.getItem(key) ?? 'null');
    if (
      value &&
      (value.group === null || typeof value.group === 'string') &&
      Number.isSafeInteger(value.row) &&
      value.row >= 0
    )
      return {
        group: value.group,
        row: value.row,
        hidden: Array.isArray(value.hidden) ? value.hidden.filter((x: unknown) => typeof x === 'string') : [],
        limit: Number.isSafeInteger(value.limit) && value.limit > 0 ? value.limit : undefined,
      };
  } catch {
    /* Private browsing may disable preference storage. */
  }
  return { group: null, row: 0 };
}

function InteractiveAnswer({
  spec,
  title,
  preferenceKey,
  header = true,
  height,
  onFollowUp,
}: {
  spec: VisualSpec;
  title: string;
  preferenceKey: string;
  header?: boolean;
  height?: number | null;
  onFollowUp?: (message: string) => void;
}) {
  const { t, formatNumber } = useI18n('flow-chat');
  const [view, setView] = useState(() => readPreferences(preferenceKey));
  const [saveFailed, setSaveFailed] = useState(false);
  const [showData, setShowData] = useState(false);
  const update = (next: ReaderState) => {
    setView(next);
    try {
      localStorage.setItem(preferenceKey, JSON.stringify(next));
      setSaveFailed(false);
    } catch {
      setSaveFailed(true);
    }
  };
  const filter = spec.filterColumn;
  const groups = useMemo(
    () => (filter ? [...new Set(spec.rows.map((row) => String(row[spec.columns.indexOf(filter)])))] : []),
    [spec, filter],
  );
  const group = view.group !== null && groups.includes(view.group) ? view.group : null;
  const table = useMemo(() => filterVisualTable(spec, group), [spec, group]);
  const pageSize = spec.options?.pageSize ?? 10;
  const ranked = useMemo(() => {
    if (spec.kind !== 'ranking') return table;
    const rows =
      spec.options?.sortOrder === 'original'
        ? table.rows
        : [...table.rows].sort(
            (a, b) => (Number(b[1]) - Number(a[1])) * (spec.options?.sortOrder === 'ascending' ? -1 : 1),
          );
    return { ...table, rows: rows.slice(0, view.limit ?? pageSize) };
  }, [table, view.limit, pageSize, spec]);
  const index = Math.min(view.row, table.rows.length - 1);
  const selected = table.rows[index];
  const display = (value: string | number | null) =>
    typeof value === 'number' ? formatNumber(value, { maximumSignificantDigits: 12 }) : (value ?? '—');
  const renderTable = () => (
    <div className="visual-answer__table" data-openbitfun-component="generative-widget" data-openbitfun-part="app">
      <table>
        <thead>
          <tr>
            {table.columns.map((col) => (
              <th scope="col" key={col}>
                {col}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {table.rows.map((row, i) => (
            <tr key={i}>
              {row.map((v, j) => (
                <td key={j}>{display(v)}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
  const pickRow = (i: number) => update({ ...view, group, row: i });
  const isTable = spec.kind === 'table' || spec.kind === 'stack-trace';
  function renderBody() {
    switch (spec.kind) {
      case 'table':
        return renderTable();
      case 'stack-trace':
        return <VisualStackTrace {...table} selected={index} select={pickRow} />;
      case 'calendar':
        return <VisualCalendar key={group ?? ''} {...table} />;
      case 'simulation':
        return (
          <>
            <label>
              {table.columns[0]} · {display(selected[0])}
              <input
                type="range"
                min={0}
                max={table.rows.length - 1}
                value={index}
                aria-valuetext={display(selected[0])}
                onChange={(e) => pickRow(Number(e.target.value))}
              />
            </label>
            <dl aria-live="polite">
              {selected.slice(1).map((value, i) => (
                <div key={i}>
                  <dt>{table.columns[i + 1]}</dt>
                  <dd>{display(value)}</dd>
                </div>
              ))}
            </dl>
          </>
        );
      default:
        return (
          <Suspense fallback={<span role="status">{t('visualize.loading')}</span>}>
            <VisualChart
              height={height}
              title={title}
              spec={spec}
              table={ranked}
              hidden={view.hidden ?? emptyNames}
              onLegend={(hidden) => update({ ...view, hidden })}
              onSelect={(i) => pickRow(table.rows.indexOf(ranked.rows[i]))}
            />
          </Suspense>
        );
    }
  }
  return (
    <figure
      className="visual-answer"
      data-openbitfun-component="generative-widget"
      data-openbitfun-part="application"
    >
      {header && <figcaption data-openbitfun-component="generative-widget" data-openbitfun-part="caption">{title}</figcaption>}
      {filter && (
        <label>
          {filter}{' '}
          <select
            value={group === null ? 0 : groups.indexOf(group) + 1}
            onChange={(e) => update({ ...view, group: groups[Number(e.target.value) - 1] ?? null, row: 0 })}
          >
            <option value={0}>{t('visualize.all')}</option>
            {groups.map((name, i) => (
              <option key={name} value={i + 1}>
                {name}
              </option>
            ))}
          </select>
        </label>
      )}
      {renderBody()}
      {spec.kind === 'ranking' && ranked.rows.length < table.rows.length && (
        <button type="button" onClick={() => update({ ...view, limit: ranked.rows.length + pageSize })}>
          {t('trajectory.showMore')}
        </button>
      )}
      {!isTable && (
        <details onToggle={(event) => setShowData(event.currentTarget.open)}>
          <summary>{t('visualize.data')}</summary>
          {showData && renderTable()}
        </details>
      )}
      {spec.followUp && onFollowUp && (
        <div className="visual-answer__controls" data-openbitfun-component="generative-widget" data-openbitfun-part="toolbar">
          {spec.kind !== 'simulation' && (
            <label>
              {t('visualize.selection')}{' '}
              <select value={index} onChange={(e) => pickRow(Number(e.target.value))}>
                {table.rows.map((row, i) => (
                  <option value={i} key={i}>
                    {display(row[0])}
                  </option>
                ))}
              </select>
            </label>
          )}
          <button
            type="button"
            onClick={() =>
              onFollowUp(
                [
                  spec.followUp!,
                  title,
                  ...(filter && group !== null ? [filter + ' = ' + group] : []),
                  ...selected.map((value, i) => table.columns[i] + ' = ' + display(value)),
                ].join('\n'),
              )
            }
          >
            {t('visualize.followUp')}
          </button>
        </div>
      )}
      {saveFailed && <p role="status">{t('visualize.saveFailed')}</p>}
    </figure>
  );
}

function ChartGroup({
  doc,
  storageKey,
  follow,
}: {
  doc: Extract<VisualDocument, { kind: 'charts' }>;
  storageKey: string;
  follow?: (m: string) => void;
}) {
  const { t } = useI18n('flow-chat');
  const { state, update, failed } = useWidgetState(storageKey);
  const filter = doc.filter,
    selected = state.all !== '1' && filter?.groups.includes(state.group) ? state.group : undefined;
  const charts = useMemo(() => {
    if (!filter || selected === undefined) return doc.charts;
    try {
      return filter.configs.map((c) => parseBoundChart(c, { field: filter.field, value: selected }));
    } catch {
      return null;
    }
  }, [doc, filter, selected]);
  return (
    <section aria-label={doc.title}>
      {filter && (
        <label>
          {filter.field}{' '}
          <select
            value={selected === undefined ? 0 : filter.groups.indexOf(selected) + 1}
            onChange={(e) =>
              update({
                group: filter.groups[Number(e.target.value) - 1] ?? '',
                all: e.target.value === '0' ? '1' : '0',
              })
            }
          >
            <option value={0}>{t('visualize.all')}</option>
            {filter.groups.map((g, i) => (
              <option value={i + 1} key={g}>
                {g}
              </option>
            ))}
          </select>
        </label>
      )}
      {charts ? (
        charts.map((c) => (
          <div key={c.id}>
            {c.showHeader !== false && c.description && <p>{c.description}</p>}
            {c.empty ? (
              <p>
                {c.title} · {t('visualize.noData')}
              </p>
            ) : (
              <InteractiveAnswer
                key={c.id}
                title={c.title}
                height={c.height}
                header={c.showHeader !== false}
                spec={c.spec}
                preferenceKey={storageKey + JSON.stringify(c.id)}
                onFollowUp={follow}
              />
            )}
            {c.summary && <p>{c.summary}</p>}
          </div>
        ))
      ) : (
        <p role="status">{t('visualize.unavailable')}</p>
      )}
      {failed && <p role="status">{t('visualize.saveFailed')}</p>}
    </section>
  );
}

export default function VisualAnswer({
  uri,
  textId,
  turnId,
  summary,
  wide = false,
}: {
  uri: string;
  textId: string;
  turnId?: string;
  summary: string;
  wide?: boolean;
}) {
  const { t } = useI18n('flow-chat');
  const { sessionId, activeSessionOverride } = useFlowChatContext();
  const view = useConversationViewScope();
  const surface = useSyncExternalStore(onSurfaceActivated, getActiveSurfaceScope);
  const owner = activeSessionOverride?.sessionId ?? sessionId;
  const current = !view || (view.surfaceId === surface.surfaceId && view.sessionId === owner);
  const getSession = useCallback(
    () =>
      current && owner ? (activeSessionOverride ?? flowChatStore.getState().sessions.get(owner)) : undefined,
    [current, owner, activeSessionOverride],
  );
  const subscribe = useCallback(
    (notify: () => void) =>
      owner && !activeSessionOverride
        ? flowChatStore.subscribeSelector((state) => state.sessions.get(owner), notify)
        : () => {},
    [owner, activeSessionOverride],
  );
  const getWrite = useCallback(
    () => {
      const session = getSession();
      const write = findVisualWrite(session, textId, uri, turnId);
      if (write) return write;
      for (const turn of session?.dialogTurns ?? []) {
        if (turnId && turn.id !== turnId) continue;
        const reply = publishedFigureReply(turn, textId, uri);
        if (reply) return reply;
      }
    },
    [getSession, textId, uri, turnId],
  );
  const item = useSyncExternalStore(subscribe, getWrite);
  const getRevision = useCallback(
    () => findVisualWrite(getSession(), textId, uri, turnId, 'revision') ?? item,
    [getSession, textId, uri, turnId, item],
  );
  const revision = useSyncExternalStore(subscribe, getRevision);
  const getCheckedBindings = useCallback((): BoundValues | undefined => {
    for (const turn of getSession()?.dialogTurns ?? []) {
      if ((!turnId || turn.id === turnId) && publishedFigureReply(turn, textId, uri)) {
        return turn.publishedFigures?.[uri]?.bindings;
      }
    }
    const receipt = revision?.type === 'tool' ? revision.toolResult?.result : undefined;
    if (receipt?.visualization_checked === true) return receipt.visualization_bindings;
  }, [getSession, textId, turnId, uri, revision]);
  const checkedBindings = useSyncExternalStore(subscribe, getCheckedBindings);
  const getSource = useCallback(
    () => readVisualRevision(getSession(), revision, textId, uri),
    [getSession, revision, textId, uri],
  );
  const html = useSyncExternalStore(subscribe, getSource);
  const doc = useMemo(() => {
    if (html && isCodexVisualUri(uri)) return { kind: 'codex' as const, html };
    return html ? parseVisualDocument(html) : null;
  }, [html, uri]);
  if (!doc || doc.kind === 'invalid' || !owner || !item)
    return (
      <p>
        {summary} <small role="status">{t('visualize.unavailable')}</small>
      </p>
    );
  if (doc.kind === 'static')
    return (
      <Suspense fallback={<p>{summary}</p>}>
        <div className={doc.hasControls ? 'visual-answer__interactive-document' : undefined}>
          <StaticDocument source={{ html: doc.html, css: inlineDocumentCss + staticFigureTypography }} />
        </div>
      </Suspense>
    );
  const key = surface.key('visualize.v1', owner, textId, (item.type === 'tool' ? item.toolCall.id : item.id), uri);
  const follow = activeSessionOverride
    ? undefined
    : (message: string) => {
        if (surface.isCurrent())
          window.dispatchEvent(
            new CustomEvent('fill-chat-input', { detail: { message, sessionId: owner, append: true } }),
          );
      };
  if (doc.kind === 'charts') return <ChartGroup key={key} doc={doc} storageKey={key} follow={follow} />;
  if (doc.kind === 'codex') {
    const turn = getSession()?.dialogTurns.find((t) =>
      turnId ? t.id === turnId : t.modelRounds.some((r) => r.items.includes(item)),
    );
    const round = turn?.modelRounds.find((r) => r.items.some((i) => i.type === 'text' && i.id === textId));
    if (!turn || !round)
      return (
        <p>
          {summary} <small role="status">{t('visualize.unavailable')}</small>
        </p>
      );
    const deferred = turn.publishedFigures?.[uri]?.inspectionDeferred ||
      (revision?.type === 'tool' && revision.toolResult?.result?.visualization_check_deferred === true);
    const stableKey = surface.key('visualize.v2', owner, turn.id, round.id, (item.type === 'tool' ? item.toolCall.id : item.id), uri);
    return (
      <Suspense fallback={<p>{summary}</p>}>
        <CodexFigure
          key={stableKey}
          storageKey={stableKey}
          legacyKey={key}
          roundId={round.id}
          html={doc.html}
          title={summary}
          wide={wide}
          follow={follow}
          uri={uri}
          checkedBindings={checkedBindings}
          deferredRequest={deferred ? turn.userMessage.content : undefined}
          write={revision ?? item}
          getSession={getSession}
          subscribe={subscribe}
        />
      </Suspense>
    );
  }
  if (doc.kind === 'widget')
    return <WidgetView key={key} data={doc.widget} title={doc.title} storageKey={key} follow={follow} />;
  return (
    <InteractiveAnswer key={key} title={doc.title} spec={doc.spec} preferenceKey={key} onFollowUp={follow} />
  );
}
