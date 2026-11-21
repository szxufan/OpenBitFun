import { useId, useMemo, useState } from 'react';
import { useI18n } from '@/infrastructure/i18n';
import { analyzeSamples, analyzeSpans, type Widget, type WidgetOf } from './widgets';
import { LayerDiagram } from './LayerDiagram';
import { IllustratedPanel } from './IllustratedPanel';

// One bounded object per artifact keeps navigation independent of sessions on other hosts.
export function useWidgetState(key: string) {
  const [state, setState] = useState<Record<string, string>>(() => {
    try {
      const value: unknown = JSON.parse(localStorage.getItem(key) ?? '{}');
      if (
        value &&
        typeof value === 'object' &&
        !Array.isArray(value) &&
        Object.values(value).every((v) => typeof v === 'string')
      )
        return value as Record<string, string>;
    } catch {
      /* Storage may be unavailable in private windows. */
    }
    return {};
  });
  const [failed, setFailed] = useState(false);
  function update(patch: Record<string, string>) {
    const next = { ...state, ...patch };
    setState(next);
    try {
      localStorage.setItem(key, JSON.stringify(next));
      setFailed(false);
    } catch {
      setFailed(true);
    }
  }
  return { state, update, failed };
}
type Prefs = ReturnType<typeof useWidgetState>;
function clockMinutes(time: string) {
  const [h, m] = time.split(':').map(Number);
  return h * 60 + m;
}
function calendarDate(anchor: Date) {
  return [
    anchor.getFullYear(),
    String(anchor.getMonth() + 1).padStart(2, '0'),
    String(anchor.getDate()).padStart(2, '0'),
  ].join('-');
}
function Calendar({ data, state, update }: { data: WidgetOf<'calendar'> } & Prefs) {
  const { t, formatDate, currentLanguage } = useI18n('flow-chat');
  const view = ['day', 'week', 'month'].includes(state.view) ? state.view : data.view;
  const anchor = new Date((/^\d{4}-\d\d-\d\d$/.test(state.date) ? state.date : data.date) + 'T12:00');
  const start = new Date(anchor);
  // Sunday in the US, Monday in the Chinese locales currently supported by the UI.
  if (view === 'week')
    start.setDate(start.getDate() - ((start.getDay() + (currentLanguage === 'en-US' ? 0 : 6)) % 7));
  if (view === 'month') start.setDate(1);
  const days =
    view === 'day'
      ? 1
      : view === 'week'
        ? 7
        : new Date(start.getFullYear(), start.getMonth() + 1, 0).getDate();
  const from = clockMinutes(data.start),
    to = clockMinutes(data.end);
  const move = (step: number) => {
    const d = new Date(start);
    if (view === 'month') d.setMonth(d.getMonth() + step);
    else d.setDate(d.getDate() + step * days);
    update({ date: calendarDate(d), event: '' });
  };
  const selected = data.events[Number(state.event ?? -1)];
  let count = 0;
  const sections = Array.from({ length: days }, (_, i) => {
    const d = new Date(start);
    d.setDate(d.getDate() + i);
    const day = calendarDate(d);
    const items = data.events
      .map((event, index) => ({ event, index }))
      .filter(
        ({ event }) =>
          (event.date ?? data.date) === day &&
          (view === 'month' || (clockMinutes(event.start) < to && clockMinutes(event.end) > from)),
      );
    count += items.length;
    const timed = view !== 'month';
    const range = to - from;
    const lanes: number[] = [];
    const arranged = [...items]
      .sort((a, b) => a.event.start.localeCompare(b.event.start))
      .map((item) => {
        const begin = Math.max(from, clockMinutes(item.event.start)),
          end = Math.min(to, clockMinutes(item.event.end));
        let lane = lanes.findIndex((stop) => stop <= begin);
        if (lane < 0) lane = lanes.length;
        lanes[lane] = end;
        return { ...item, begin, end, lane };
      });
    return (
      <section key={day}>
        <strong>{formatDate(d, { month: 'short', day: 'numeric', weekday: 'short' })}</strong>
        <div className={timed ? 'visual-answer__agenda' : undefined}>
          {timed &&
            data.now &&
            day === data.date &&
            clockMinutes(data.now) >= from &&
            clockMinutes(data.now) < to && (
              <small
                className="visual-answer__now"
                style={{ top: ((clockMinutes(data.now) - from) / range) * 100 + '%' }}
              >
                {t('visualize.now')} {data.now}
              </small>
            )}
          {arranged.map(({ event, index, begin, end, lane }) => (
            <button
              type="button"
              key={index}
              style={
                timed
                  ? {
                      top: ((begin - from) / range) * 100 + '%',
                      height: ((end - begin) / range) * 100 + '%',
                      insetInlineStart: (lane / lanes.length) * 100 + '%',
                      width: 100 / lanes.length + '%',
                    }
                  : undefined
              }
              title={[event.title, event.start + '–' + event.end, event.detail].filter(Boolean).join('\n')}
              aria-pressed={state.event === String(index)}
              data-tone={event.tone}
              data-past={data.now && day === data.date && event.end <= data.now}
              onClick={() => update({ event: String(index) })}
            >
              {event.title} · {event.start}–{event.end}
              {event.video ? ' ▷' : ''}
            </button>
          ))}
        </div>
      </section>
    );
  });
  return (
    <>
      <div className="visual-answer__controls">
        <button type="button" aria-label={t('visualize.previous')} onClick={() => move(-1)}>
          ‹
        </button>
        <select
          aria-label={t('visualize.view')}
          value={view}
          onChange={(e) => update({ view: e.target.value, event: '' })}
        >
          {(['day', 'week', 'month'] as const).map((v) => (
            <option key={v} value={v}>
              {t('visualize.' + v)}
            </option>
          ))}
        </select>
        <button type="button" aria-label={t('visualize.next')} onClick={() => move(1)}>
          ›
        </button>
        {data.timeZone}
      </div>
      <div className="visual-answer__days">{sections}</div>
      {!count && data.emptyLabel && <p>{data.emptyLabel}</p>}
      {selected && state.event !== '' && (
        <output>
          {[selected.title, selected.start + '–' + selected.end, selected.detail].filter(Boolean).join(' · ')}
        </output>
      )}
    </>
  );
}
function Trace({ data, state, update }: { data: WidgetOf<'stack-trace'> } & Prefs) {
  const { t, formatNumber } = useI18n('flow-chat');
  const stacks = [{ ...data.error, frames: data.frames }, ...data.causes];
  return (
    <>
      {stacks.map((stack, si) => {
        const groups: {
          runtime: boolean;
          entries: { frame: (typeof stack.frames)[number]; id: string }[];
        }[] = [];
        stack.frames.forEach((frame, i) => {
          const runtime = frame.kind === 'runtime';
          if (!groups.length || !runtime || !groups.at(-1)!.runtime || frame.async)
            groups.push({ runtime, entries: [] });
          groups.at(-1)!.entries.push({ frame, id: frame.id ?? si + '/' + i });
        });
        return (
          <section key={si}>
            <strong>
              {stack.name} · {stack.message}
            </strong>
            {stack.frames.length === 0 && <pre>{stack.stack}</pre>}
            {groups.map(({ entries, runtime }, gi) => {
              const key = 'group/' + si + '/' + gi;
              const list = (
                <ol className="visual-answer__frames">
                  {entries.map(({ frame, id }) => (
                    <li key={id}>
                      {frame.async && <small>{t('visualize.asyncBoundary')}</small>}
                      <button
                        type="button"
                        aria-pressed={state.frame === id}
                        onClick={() => update({ frame: id })}
                      >
                        {[frame.name, frame.file, frame.line, frame.column]
                          .filter((v) => v !== undefined)
                          .join(' · ')}
                      </button>
                      {state.frame === id && frame.source && (
                        <pre>
                          {frame.source.lines.map((line, i) => (
                            <span key={i} data-current={frame.line === frame.source!.startLine + i}>
                              {formatNumber(frame.source!.startLine + i, { useGrouping: false })}
                              {'  '}
                              {line}
                              {'\n'}
                            </span>
                          ))}
                        </pre>
                      )}
                    </li>
                  ))}
                </ol>
              );
              return runtime ? (
                <div key={key}>
                  <button
                    type="button"
                    aria-expanded={state[key] === '1'}
                    onClick={() => update({ [key]: state[key] === '1' ? '0' : '1' })}
                  >
                    {t('visualize.runtimeFrames')} ({entries.length})
                  </button>
                  {state[key] === '1' && list}
                </div>
              ) : (
                <div key={key}>{list}</div>
              );
            })}
          </section>
        );
      })}
    </>
  );
}
function Panels({
  data,
  state,
  update,
  follow,
}: { data: WidgetOf<'tabs'>; follow?: (message: string) => void } & Prefs) {
  const { t, formatNumber } = useI18n('flow-chat');
  const prefix = useId(),
    index = Math.min(
      data.panels.length - 1,
      Math.max(0, Number.isSafeInteger(Number(state.tab)) ? Number(state.tab) : 0),
    ),
    panel = data.panels[index];
  const values = (panel.fields ?? []).map((field, i) => {
    const raw = state[index + '/' + i];
    if ('options' in field) return field.options.includes(raw) ? raw : field.options[0];
    if (field.type === 'text') return raw?.slice(0, 2048) ?? field.value;
    if (field.type === 'checkbox') return ['true', 'false'].includes(raw) ? raw : String(field.value);
    return raw !== undefined &&
      raw !== '' &&
      Number.isFinite(Number(raw)) &&
      Number(raw) >= field.min &&
      Number(raw) <= field.max
      ? raw
      : String(field.value);
  });
  return (
    <>
      {data.panels.length > 1 && <div role="tablist" className="visual-answer__controls">
        {data.panels.map((p, i) => (
          <button
            type="button"
            role="tab"
            id={prefix + 't' + i}
            aria-controls={prefix + 'p'}
            aria-selected={index === i}
            tabIndex={index === i ? 0 : -1}
            key={i}
            onClick={() => update({ tab: String(i) })}
            onKeyDown={(e) => {
              const next =
                e.key === 'Home'
                  ? 0
                  : e.key === 'End'
                    ? data.panels.length - 1
                    : e.key === 'ArrowRight'
                      ? (index + 1) % data.panels.length
                      : e.key === 'ArrowLeft'
                        ? (index + data.panels.length - 1) % data.panels.length
                        : undefined;
              if (next !== undefined) {
                e.preventDefault();
                update({ tab: String(next) });
                document.getElementById(prefix + 't' + next)?.focus();
              }
            }}
          >
            {p.title}
          </button>
        ))}
      </div>}
      <section
        className="visual-answer__panel"
        role={data.panels.length > 1 ? 'tabpanel' : 'group'}
        id={prefix + 'p'}
        aria-labelledby={data.panels.length > 1 ? prefix + 't' + index : undefined}
        aria-label={data.panels.length === 1 ? panel.title : undefined}
      >
        {panel.text && <p>{panel.text}</p>}
        <IllustratedPanel
          panel={panel} selection={state['variant/' + index]}
          onChange={(choice) => update({ ['variant/' + index]: String(choice) })}
        />
        {panel.progress && (
          <label>
            {panel.progress.label} · {formatNumber(panel.progress.value)} / {formatNumber(panel.progress.max)}{' '}
            <progress value={panel.progress.value} max={panel.progress.max} />
          </label>
        )}
        {panel.columns && (
          <div className="visual-answer__table">
            <table>
              <thead>
                <tr>
                  {panel.columns.map((c, i) => (
                    <th key={i}>{c}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {panel.rows?.map((r, i) => (
                  <tr key={i}>
                    {r.map((v, j) => (
                      <td key={j}>{typeof v === 'number' ? formatNumber(v) : String(v ?? '—')}</td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {panel.fields?.map((field, i) => {
          const value = values[i],
            change = (v: string) => update({ [index + '/' + i]: v });
          if ('options' in field) {
            if (field.type === 'radio')
              return (
                <fieldset key={i}>
                  <legend>{field.label}</legend>
                  {field.options.map((v) => (
                    <label key={v}>
                      <input
                        type="radio"
                        name={prefix + i}
                        checked={value === v}
                        onChange={() => change(v)}
                      />
                      {v}
                    </label>
                  ))}
                </fieldset>
              );
            return (
              <label key={i}>
                {field.label}{' '}
                <select value={value} onChange={(e) => change(e.target.value)}>
                  {field.options.map((v, j) => (
                    <option key={j}>{v}</option>
                  ))}
                </select>
              </label>
            );
          }
          if (field.type === 'checkbox')
            return (
              <label key={i}>
                <input
                  type="checkbox"
                  checked={value === 'true'}
                  onChange={(e) => change(String(e.target.checked))}
                />
                {field.label}
              </label>
            );
          if (field.type === 'text')
            return (
              <label key={i}>
                {field.label}{' '}
                <input value={value} maxLength={2048} onChange={(e) => change(e.target.value)} />
              </label>
            );
          return (
            <label key={i}>
              {field.label} {field.type === 'range' && formatNumber(Number(value))}
              <input
                type={field.type}
                min={field.min}
                max={field.max}
                step={field.step ?? 'any'}
                value={value}
                onChange={(e) => change(e.target.value)}
              />
            </label>
          );
        })}
        {follow && panel.followUp && (
          <button
            type="button"
            onClick={() =>
              follow(
                [
                  panel.followUp!,
                  panel.title,
                  ...(panel.fields ?? []).map((f, i) => f.label + ' = ' + values[i]),
                ].join('\n'),
              )
            }
          >
            {t('visualize.followUp')}
          </button>
        )}
      </section>
    </>
  );
}
type Timing = WidgetOf<'request-waterfall'> | WidgetOf<'flame-chart'>;
function timelineModel(data: Timing) {
  if (data.kind === 'request-waterfall') {
    const spans = analyzeSpans(data);
    const byId = new Map(spans.map((s) => [s.id, s]));
    const rows = spans.map((s) => {
      const parents: string[] = [];
      let p = s.parentId;
      while (p) {
        parents.push(p);
        p = byId.get(p)?.parentId;
      }
      return {
        id: s.id,
        name: s.name,
        start: s.startMs,
        duration: s.durationMs,
        self: s.direct,
        uncovered: s.uncovered,
        parents,
        context: [s.service, s.status].filter(Boolean).join(' · '),
        attributes: s.attributes,
        hasChildren: s.hasChildren,
        depth: parents.length,
      };
    });
    return { rows, end: data.durationMs ?? Math.max(...spans.map((s) => s.end)), functions: undefined };
  }
  const { calls, totals, elapsed } = analyzeSamples(data);
  const frames = new Map(data.frames.map((f) => [f.id, f]));
  const rows = calls.map((c) => ({
    id: String(c.id),
    name: frames.get(c.frameId)!.name,
    start: c.start,
    duration: c.duration,
    self: c.self,
    uncovered: [] as [number, number][],
    parents: [] as string[],
    context: frames.get(c.frameId)?.category,
    attributes: undefined,
    hasChildren: false,
    depth: c.depth,
  }));
  return {
    rows,
    end: elapsed,
    functions: [...totals]
      .map(([id, times]) => ({ id, name: frames.get(id)!.name, ...times }))
      .sort((a, b) => b.self - a.self),
  };
}
function Timeline({ data, state, update }: { data: Timing } & Prefs) {
  const { formatNumber, t } = useI18n('flow-chat');
  const model = useMemo(() => timelineModel(data), [data]);
  const flame = data.kind === 'flame-chart';
  const fmt = (v: number) => formatNumber(v, { maximumFractionDigits: 3 });
  const ms = (v: number) => fmt(v) + ' ms';
  const zoom = Math.min(128, Math.max(1, Number(state.zoom) || 1)),
    width = (model.end || 1) / zoom;
  const start = Math.max(0, Math.min(Number(state.start) || 0, Math.max(0, model.end - width)));
  const changeRange = (scale: number, offset: number) =>
    update({
      zoom: String(scale),
      start: String(Math.max(0, Math.min(offset, model.end - (model.end || 1) / scale))),
      page: '0',
    });
  const query = (state.query ?? '').toLowerCase();
  const collapsed = new Set(state.collapsed ? readList(state.collapsed) : []);
  const critical = useMemo(() => {
    if (data.kind !== 'request-waterfall') return new Set<string>();
    if (data.criticalPath) return new Set(data.criticalPath);
    // Longest nested path by direct time. Timing is an estimate of causality.
    const byId = new Map(model.rows.map((r) => [r.id, r]));
    let longest = 0,
      path: string[] = [];
    for (const r of model.rows) {
      const score = r.parents.reduce((sum, id) => sum + (byId.get(id)?.self ?? 0), r.self);
      if (score > longest) {
        longest = score;
        path = [...r.parents, r.id];
      }
    }
    return new Set(path);
  }, [model, data]);
  const visible = model.rows.filter(
    (r) =>
      r.start + r.duration >= start &&
      r.start <= start + width &&
      !r.parents.some((id) => collapsed.has(id)) &&
      (r.name + ' ' + (r.context ?? '')).toLowerCase().includes(query) &&
      (state.critical !== '1' || critical.has(r.id)),
  );
  if (!flame)
    visible.sort((a, b) =>
      state.sort === 'duration'
        ? b.duration - a.duration
        : state.sort === 'self'
          ? b.self - a.self
          : a.start - b.start,
    );
  const page = Math.max(0, Math.min(Number(state.page) || 0, Math.ceil(visible.length / 100) - 1));
  const pageRows = visible.slice(page * 100, (page + 1) * 100);
  const detail = model.rows.find((r) => r.id === state.selected);
  return (
    <>
      <div className="visual-answer__controls">
        <label>
          {t('shared:tools.search')}{' '}
          <input
            type="search"
            value={state.query ?? ''}
            onChange={(e) => update({ query: e.target.value, page: '0' })}
          />
        </label>
        {!flame && (
          <label>
            {t('visualize.sort')}{' '}
            <select
              value={state.sort ?? 'start'}
              onChange={(e) => update({ sort: e.target.value, page: '0' })}
            >
              <option value="start">{t('visualize.start')}</option>
              <option value="duration">{t('visualize.total')}</option>
              <option value="self">{t('visualize.direct')}</option>
            </select>
          </label>
        )}
        {!flame && (
          <label>
            <input
              type="checkbox"
              checked={state.direct === '1'}
              onChange={(e) => update({ direct: e.target.checked ? '1' : '0' })}
            />
            {t('visualize.direct')}
          </label>
        )}
        {critical.size > 0 && (
          <label>
            <input
              type="checkbox"
              checked={state.critical === '1'}
              onChange={(e) => update({ critical: e.target.checked ? '1' : '0', page: '0' })}
            />
            {data.kind === 'request-waterfall' && data.criticalPath
              ? t('visualize.critical')
              : t('visualize.estimatedPath')}
          </label>
        )}
        <button
          type="button"
          aria-label={t('visualize.zoomIn')}
          disabled={zoom >= 128}
          onClick={() => changeRange(zoom * 2, start + width / 4)}
        >
          ＋
        </button>
        <button
          type="button"
          aria-label={t('visualize.zoomOut')}
          disabled={zoom <= 1}
          onClick={() => changeRange(zoom / 2, start - width / 2)}
        >
          −
        </button>
        <label>
          {t('visualize.range')}{' '}
          <input
            type="range"
            min={0}
            max={Math.max(0, model.end - width)}
            step="any"
            value={start}
            onChange={(e) => changeRange(zoom, Number(e.target.value))}
          />
        </label>
        <span>
          {ms(start)}–{ms(start + width)}
        </span>
      </div>
      {data.kind === 'flame-chart' && data.threadName && <strong>{data.threadName}</strong>}
      {flame && (
        <div className="visual-answer__flame" aria-label={t('visualize.functions')}>
          {Array.from({ length: Math.max(0, ...pageRows.map((r) => r.depth + 1)) }, (_, depth) => (
            <div className="visual-answer__level" key={depth}>
              {pageRows
                .filter((r) => r.depth === depth)
                .map((r) => {
                  const left = Math.max(0, ((r.start - start) / width) * 100),
                    right = Math.min(100, ((r.start + r.duration - start) / width) * 100);
                  if (right <= left) return null;
                  return (
                    <button
                      type="button"
                      key={r.id}
                      title={r.name + ' · ' + ms(r.duration)}
                      aria-pressed={r.id === state.selected}
                      style={{ insetInlineStart: left + '%', width: right - left + '%' }}
                      onClick={() => update({ selected: r.id })}
                    >
                      {r.name}
                    </button>
                  );
                })}
            </div>
          ))}
        </div>
      )}
      {!flame && (
        <div className="visual-answer__timeline">
          {pageRows.map((r) => {
            const left = Math.max(0, ((r.start - start) / width) * 100),
              right = Math.min(100, ((r.start + r.duration - start) / width) * 100);
            return (
              <div key={r.id} className="visual-answer__span">
                {r.hasChildren && (
                  <button
                    type="button"
                    aria-label={r.name}
                    aria-expanded={!collapsed.has(r.id)}
                    onClick={() => {
                      const ids = new Set(collapsed);
                      if (ids.has(r.id)) ids.delete(r.id);
                      else ids.add(r.id);
                      update({ collapsed: JSON.stringify([...ids]), page: '0' });
                    }}
                  >
                    {collapsed.has(r.id) ? '▸' : '▾'}
                  </button>
                )}
                <button
                  type="button"
                  aria-pressed={r.id === state.selected}
                  title={r.name + ' · ' + ms(r.duration)}
                  onClick={() => update({ selected: r.id })}
                >
                  <span style={{ paddingInlineStart: Math.min(r.depth, 8) * 5 }}>{r.name}</span>
                  <span className="visual-answer__track" aria-hidden="true">
                    {state.direct === '1' ? (
                      r.uncovered.map(([a, b], i) => {
                        const begin = Math.max(0, ((a - start) / width) * 100),
                          end = Math.min(100, ((b - start) / width) * 100);
                        return begin < end ? (
                          <i key={i} style={{ insetInlineStart: begin + '%', width: end - begin + '%' }} />
                        ) : null;
                      })
                    ) : (
                      <i style={{ insetInlineStart: left + '%', width: Math.max(0, right - left) + '%' }} />
                    )}
                  </span>
                </button>
              </div>
            );
          })}
        </div>
      )}
      {visible.length > 100 && (
        <div>
          <button type="button" disabled={!page} onClick={() => update({ page: String(page - 1) })}>
            {t('visualize.previous')}
          </button>{' '}
          {fmt(page + 1)} / {fmt(Math.ceil(visible.length / 100))}{' '}
          <button
            type="button"
            disabled={(page + 1) * 100 >= visible.length}
            onClick={() => update({ page: String(page + 1) })}
          >
            {t('visualize.next')}
          </button>
        </div>
      )}
      {detail && (
        <output>
          {detail.name} · {t('visualize.start')} {ms(detail.start)} · {t('visualize.end')}{' '}
          {ms(detail.start + detail.duration)} · {t('visualize.total')} {ms(detail.duration)} ·{' '}
          {t('visualize.direct')} {ms(detail.self)} {detail.context}
          {Object.entries(detail.attributes ?? {}).map(([k, v]) => (
            <span key={k}>
              {' '}
              · {k} = {String(v)}
            </span>
          ))}
        </output>
      )}
      {model.functions && (
        <details>
          <summary>{t('visualize.functions')}</summary>
          <div className="visual-answer__table">
            <table>
              <thead>
                <tr>
                  <th>{t('visualize.functions')}</th>
                  <th>{t('visualize.total')}</th>
                  <th>{t('visualize.direct')}</th>
                </tr>
              </thead>
              <tbody>
                {model.functions
                  .filter((f) => f.name.toLowerCase().includes(query))
                  .map((f) => (
                    <tr key={f.id}>
                      <td>{f.name}</td>
                      <td>{ms(f.inclusive)}</td>
                      <td>{ms(f.self)}</td>
                    </tr>
                  ))}
              </tbody>
            </table>
          </div>
        </details>
      )}
    </>
  );
}
function readList(raw: string): string[] {
  try {
    const v: unknown = JSON.parse(raw);
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
  } catch {
    return [];
  }
}
export function WidgetView({
  data,
  title,
  storageKey,
  follow,
}: {
  data: Widget;
  title: string;
  storageKey: string;
  follow?: (message: string) => void;
}) {
  const prefs = useWidgetState(storageKey);
  const { t } = useI18n('flow-chat');
  return (
    <figure
      className={`visual-answer${data.kind === 'layers' ? ' visual-layers' : data.kind === 'tabs' ? ' visual-guide' : ''}`}
      data-openbitfun-component="generative-widget"
      data-openbitfun-part="application"
    >
      <figcaption>{title}</figcaption>
      {data.description && <p>{data.description}</p>}
      {data.kind === 'layers' ? (
        <LayerDiagram data={data} title={title} selected={prefs.state.layer} onSelect={layer => prefs.update({ layer })} />
      ) : data.kind === 'calendar' ? (
        <Calendar data={data} {...prefs} />
      ) : data.kind === 'stack-trace' ? (
        <Trace data={data} {...prefs} />
      ) : data.kind === 'tabs' ? (
        <Panels data={data} follow={follow} {...prefs} />
      ) : (
        <Timeline data={data} {...prefs} />
      )}
      {prefs.failed && <p role="status">{t('visualize.saveFailed')}</p>}
    </figure>
  );
}
