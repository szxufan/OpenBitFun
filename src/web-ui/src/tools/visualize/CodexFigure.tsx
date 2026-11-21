import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { LiveResourceBinding } from './LiveResourceBinding';
import type { FlowTextItem, FlowToolItem, Session } from '@/flow_chat/types/flow-chat';
import {
  readBindings,
  readerBindingSnapshot,
  resolveBindingSnapshots,
  refreshedBinding,
  latestSessionBinding,
  type BoundValues,
} from './visualBindings';
import { z } from 'zod/v4';
import { useI18n } from '@/infrastructure/i18n';
import { useHostAppearance } from '@/infrastructure/appearance/hooks/useHostAppearance';
import { buildCodexFrame } from './codexFrameDocument';
import { watchFigureInspection, type MapPublicationCheck } from './figureInspection';
import { mapPublication } from './mapPublication';
import { readFigureStyle } from './figureAppearance';
import { readWidgetState } from './widgetStateStorage';

const label = z.string().max(256);
const common = { callback: label, label, reference: label.optional() };
const control = z.discriminatedUnion('type', [
  z.object({
    ...common,
    type: z.literal('range'),
    currentValue: z.number(),
    min: z.number(),
    max: z.number(),
    step: z.number().positive(),
  }),
  z.object({ ...common, type: z.literal('toggle'), currentValue: z.boolean() }),
  z.object({ ...common, type: z.literal('color'), currentValue: label }),
  z.object({
    ...common,
    type: z.literal('select'),
    currentValue: label,
    options: z.array(z.object({ label, value: label })).max(12),
  }),
]);
const registration = z.object({
  registrationId: label,
  targets: z.array(z.object({ id: label, label, visible: z.boolean() })).length(1),
  annotationControls: z.object({ controls: z.array(control).max(12) }),
});
type Group = z.infer<typeof registration>;
type Value = string | number | boolean;

function parseSnapshot(serialized: string) {
  if (new TextEncoder().encode(serialized).length > 16 * 1024) throw Error('Oversize state');
  const state = JSON.parse(serialized);
  if (!state || typeof state !== 'object' || Array.isArray(state)) throw Error('Invalid state');
  return { ...state, modelContent: state.modelContent ?? null, privateContent: state.privateContent ?? null };
}

export default function CodexFigure({
  html,
  title,
  wide,
  storageKey,
  legacyKey,
  roundId,
  follow,
  uri,
  write,
  checkedBindings,
  deferredRequest,
  getSession,
  subscribe,
}: {
  uri: string;
  write: FlowToolItem | FlowTextItem;
  checkedBindings?: BoundValues;
  deferredRequest?: string;
  getSession: () => Session | undefined;
  subscribe: (notify: () => void) => () => void;
  html: string;
  title: string;
  wide: boolean;
  storageKey: string;
  legacyKey: string;
  roundId: string;
  follow?: (message: string) => void;
}) {
  const { t, currentLanguage } = useI18n('flow-chat');
  const ref = useRef<HTMLIFrameElement>(null);
  const wrapper = useRef<HTMLDivElement>(null);
  const appearance = useHostAppearance(wrapper);
  const figureStyle = useMemo(
    () => readFigureStyle(wrapper.current ?? document.documentElement, appearance.scheme),
    [appearance],
  );
  const scheme = figureStyle['color-scheme'] === 'dark' ? 'dark' : 'light';
  const [height, setHeight] = useState(180);
  const [expanded, setExpanded] = useState(false);
  const [controlsOpen, setControlsOpen] = useState(false);
  const [groups, setGroups] = useState<Group[]>([]);
  const [edits, setEdits] = useState<Record<string, Value>>({});
  const [layout, setLayout] = useState(false);
  const [original, setOriginal] = useState(false);
  const dashboard = /<(?:experimental-)?dash-[a-z-]+\b|data-dashboard-block/.test(html);
  const [annotating, setAnnotating] = useState(false);
  const [annotation, setAnnotation] = useState<{
    kind: string;
    id: string;
    blockId?: string;
    label?: string;
  } | null>(null);
  const [note, setNote] = useState('');
  const [sourceLabels, setSourceLabels] = useState<{ value: string; label: string }[]>([]);
  const [sourceMetadata, setSourceMetadata] = useState<
    { name: string; title?: string; description?: string }[]
  >([]);
  const [loadError, setLoadError] = useState(false);
  const [mapNeedsConsent, setMapNeedsConsent] = useState(false);
  const [onlineRevision, setOnlineRevision] = useState<string>();
  const [action, setAction] = useState<{ href?: string; prompt?: string; title?: string } | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [ready, setReady] = useState(0);
  const port = useRef<MessagePort | null>(null);
  const followRef = useRef(follow);
  followRef.current = follow;
  const presentation = useRef<MessagePort | null>(null);
  const declarations = useMemo(() => readBindings(html), [html]);
  const readSnapshot = useMemo(() => {
    const sources = declarations ?? {};
    return readerBindingSnapshot(sources, () => resolveBindingSnapshots(sources, getSession(), write, uri), checkedBindings);
  }, [checkedBindings, declarations, getSession, write, uri]);
  const bindings = useSyncExternalStore(subscribe, readSnapshot);
  const [sourcesOpen, setSourcesOpen] = useState(false);
  const [selectedSources, setSelectedSources] = useState<string[] | null>(null);
  const [liveValues, setLiveValues] = useState<BoundValues>({});
  const [requests, setRequests] = useState<Record<string, string>>({});
  const [watching, setWatching] = useState<string[]>([]);
  const canFollow = Boolean(follow);
  useEffect(() => {
    if (!declarations || !canFollow) return;
    const timers = watching.map((name) => {
      const binding = declarations[name];
      if (binding.type !== 'library' && binding.type !== 'tool') return;
      let last = '';
      const refresh = () => {
        const next = latestSessionBinding(getSession(), name, binding, uri, write.subagentSessionId);
        if (!next || next.id === last) return;
        last = next.id;
        setLiveValues((previous) => ({ ...previous, [name]: next.data }));
      };
      refresh();
      return window.setInterval(refresh, binding.refreshIntervalSeconds * 1000);
    });
    return () => timers.forEach((t) => window.clearInterval(t));
  }, [declarations, canFollow, watching, getSession, uri, write.subagentSessionId]);
  const modelContent = useRef<unknown>(null);
  useEffect(() => {
    if (!declarations || !Object.keys(requests).length) return;
    const check = () => {
      const values: BoundValues = {};
      for (const [name, request] of Object.entries(requests)) {
        const value = refreshedBinding(getSession(), request, name, declarations[name], uri);
        if (value) values[name] = value;
      }
      if (Object.keys(values).length) {
        setLiveValues((prev) => ({ ...prev, ...values }));
        setRequests((prev) => Object.fromEntries(Object.entries(prev).filter(([name]) => !values[name])));
      }
    };
    const off = subscribe(check);
    check();
    return off;
  }, [declarations, requests, getSession, subscribe, uri]);
  useEffect(() => {
    if (ready && Object.keys(liveValues).length) presentation.current?.postMessage({ values: liveValues });
  }, [ready, liveValues]);
  const [built, setBuilt] = useState<{ html: string; document: string; version: number; publication?: MapPublicationCheck; saveMap?: () => void }>();
  const source = built?.html === html ? built.document : '';
  const [passed, setPassed] = useState(0);
  const [issue, setIssue] = useState<string | null>(null);
  useEffect(() => {
    let mounted = true;
    let state: ReturnType<typeof parseSnapshot> | null = null;
    try {
      state = parseSnapshot(readWidgetState(localStorage, storageKey, legacyKey, roundId) ?? 'null');
    } catch {
      /* First visit or private browsing. */
    }
    modelContent.current = state?.modelContent ?? null;
    const initialStyle = readFigureStyle(wrapper.current ?? document.documentElement);
    setMapNeedsConsent(false);
    setLoadError(false);
    setIssue(null);
    async function build() {
      let fragment = html;
      let saveMap: (() => void) | undefined;
      if (html.includes('data-visual-atlas')) {
        const { recoverReaderStreetMap } = await import('./regionalAtlas');
        if (!mounted) return;
        const allowNames = !!declarations && Object.values(declarations).every(entry => entry.type === 'image');
        let recovery;
        try {
          recovery = await recoverReaderStreetMap(html, storageKey, allowNames, onlineRevision === html);
        } catch (error) {
          if (mounted && error instanceof Error) setIssue(error.message);
          throw error;
        }
        if (!mounted) return;
        if (!recovery) {
          setMapNeedsConsent(true);
          return;
        }
        fragment = recovery.html;
        saveMap = recovery.save;
      }
      const publication = deferredRequest === undefined ? undefined :
        { ...await mapPublication(uri, deferredRequest, fragment), replay: true };
      const document = await buildCodexFrame(fragment, state, currentLanguage, declarations ?? {}, bindings, initialStyle);
      if (mounted) setBuilt(prev => ({ html, document, publication, saveMap, version: (prev?.version ?? 0) + 1 }));
    }
    void build().catch(() => {
      if (mounted) setLoadError(true);
    });
    return () => {
      mounted = false;
    };
  }, [html, storageKey, legacyKey, roundId, currentLanguage, attempt, declarations, bindings, onlineRevision, uri, deferredRequest]);
  const changes = useMemo(
    () =>
      groups.map((g) => ({
        registrationId: g.registrationId,
        targetId: g.targets[0].id,
        annotationControlChanges: g.annotationControls.controls
          .filter((c) => edits[c.callback] !== undefined && edits[c.callback] !== c.currentValue)
          .map((c) => ({ callback: c.callback, previousValue: c.currentValue, value: edits[c.callback] })),
      })),
    [groups, edits],
  );
  const postGlobals = useCallback(
    () =>
      presentation.current?.postMessage({
        figureStyle,
        globals: {
          theme: scheme,
          visualizationTheme: scheme,
          visualizationAnnotation: { active: controlsOpen, changes: original ? [] : changes },
          sandboxDocumentState: {
            dashboardVisible: true,
            dashboardSourcePanel: true,
            dashboardSourcesOpen: sourcesOpen,
            dashboardAnnotateMode: annotating,
            dashboardAnnotationTarget: annotation,
            dashboardEditMode: controlsOpen,
            dashboardDirectEditing: controlsOpen,
          },
        },
      }),
    [scheme, figureStyle, changes, controlsOpen, sourcesOpen, annotating, annotation, original],
  );
  useEffect(() => {
    if (ready) postGlobals();
  }, [ready, postGlobals]);
  useEffect(() => {
    const changed = () => setExpanded(document.fullscreenElement === wrapper.current);
    document.addEventListener('fullscreenchange', changed);
    return () => document.removeEventListener('fullscreenchange', changed);
  }, []);

  useLayoutEffect(() => {
    const frame = ref.current;
    if (!frame || !source) return;
    let connected = false;
    setIssue(null);
    const stopChecking = watchFigureInspection(frame, problem => {
      if (problem) setIssue(problem);
      else {
        built!.saveMap?.();
        setPassed(built!.version);
      }
    }, built?.publication);
    setGroups([]);
    setEdits({});
    setLoadError(false);
    setAction(null);
    setLayout(false);
    setControlsOpen(false);
    setOriginal(false);
    setAnnotation(null);
    setNote('');
    setAnnotating(false);
    setSourceLabels([]);
    setSourceMetadata([]);
    const receive = (event: MessageEvent) => {
      if (event.source !== frame.contentWindow || event.data?.openBitFunVisual !== 2) return;
      const { type, args } = event.data;
      if (type === 'ready' && !connected) {
        connected = true;
        const bridge = new MessageChannel();
        presentation.current = bridge.port1;
        frame.contentWindow?.postMessage({ openBitFunVisual: 2, type: 'attach' }, '*', [bridge.port2]);
        const channel = new MessageChannel();
        port.current = channel.port1;
        channel.port1.onmessage = ({ data }) => {
          if (!data || typeof data !== 'object') return;
          if (data.type === 'height' && Number.isFinite(data.height) && data.height > 0) {
            setHeight(Math.min(10000, Math.max(40, Math.ceil(data.height))));
          } else if (
            data.type === 'widget-state-write' &&
            typeof data.state === 'string' &&
            Number.isSafeInteger(data.id)
          ) {
            let ok = false;
            try {
              const state = parseSnapshot(data.state);
              localStorage.setItem(storageKey, data.state);
              modelContent.current = state.modelContent;
              ok = true;
            } catch {
              /* Reply with the upstream failure contract. */
            }
            channel.port1.postMessage({ type: 'widget-state-result', id: data.id, ok });
          } else if (
            data.type === 'follow-up' &&
            followRef.current &&
            typeof data.prompt === 'string' &&
            data.prompt.length <= 16384
          ) {
            setAction({
              prompt: data.prompt,
              title: typeof data.title === 'string' ? data.title.slice(0, 250) : undefined,
            });
          } else if (
            data.type === 'open-external' &&
            typeof data.href === 'string' &&
            data.href.length <= 4096
          ) {
            try {
              const url = new URL(data.href);
              if (url.protocol === 'https:') setAction({ href: url.href });
            } catch {
              /* Invalid destination. */
            }
          } else if (
            data.type === 'scroll-to' &&
            Number.isFinite(data.top) &&
            navigator.userActivation?.isActive
          ) {
            const scroll = frame.closest('.flow-chat-scroll, [data-overlayscrollbars-viewport]');
            const offset =
              frame.getBoundingClientRect().top + Math.max(0, Math.min(data.top, frame.offsetHeight));
            if (scroll) scroll.scrollBy({ top: offset - scroll.getBoundingClientRect().top });
            else window.scrollBy({ top: offset });
          }
        };
        frame.contentWindow?.postMessage({ type: 'codex-visualization-initialize' }, '*', [channel.port2]);
        setReady((value) => value + 1);
      } else if (type === 'resource-error') setLoadError(true);
      else if (type === 'layout') setLayout(event.data.available === true);
      else if (type === 'source-labels') {
        const tabs = z
          .array(z.object({ value: label, label }))
          .max(128)
          .safeParse(args?.labels);
        if (tabs.success) setSourceLabels(tabs.data);
      } else if (type === 'source-inspect') {
        const metadata = z
          .array(
            z.object({ name: label, title: label.optional(), description: z.string().max(512).optional() }),
          )
          .max(32)
          .safeParse(args?.metadata);
        if (metadata.success) setSourceMetadata(metadata.data);
        const id = args?.selection?.id;
        const names =
          args?.selection?.kind === 'source'
            ? [id]
            : args?.selection?.kind === 'block' && Array.isArray(args.blocks)
              ? args.blocks.find((b: { id?: string }) => b?.id === id)?.sources
              : null;
        setSelectedSources(
          Array.isArray(names) ? names.filter((n: unknown) => typeof n === 'string').slice(0, 32) : null,
        );
        setSourcesOpen(true);
      } else if (type === 'annotation' && args && event.ports[0]) {
        let result = {};
        if (args.type === 'register' || args.type === 'update') {
          const parsed = registration.safeParse(args);
          if (parsed.success) {
            const group = parsed.data;
            setGroups((previous) =>
              [...previous.filter((g) => g.registrationId !== group.registrationId), group].slice(-64),
            );
            result = { annotationControls: group.annotationControls };
          }
        } else if (args.type === 'dashboard-select' && !args.tracking) {
          const target = z
            .object({ kind: label, id: label, label: label.optional(), blockId: label.optional() })
            .safeParse(args.target);
          if (target.success) {
            setAnnotation(target.data);
            setNote('');
          }
        } else if (args.type === 'dashboard-escape') {
          setAnnotating(false);
          setAnnotation(null);
        } else if (args.type === 'dispose')
          setGroups((previous) => previous.filter((g) => g.registrationId !== args.registrationId));
        else if (args.type === 'select') setControlsOpen(true);
        else if (args.type === 'escape') setControlsOpen(false);
        event.ports[0].postMessage(result);
        event.ports[0].close();
      }
    };
    window.addEventListener('message', receive);
    frame.srcdoc = source;
    return () => {
      stopChecking();
      window.removeEventListener('message', receive);
      presentation.current?.close();
      presentation.current = null;
      port.current?.close();
      port.current = null;
    };
  }, [source, storageKey, built]);

  const verified = !!source && passed === built?.version;
  const visible = groups.filter((group) => group.targets[0].visible);
  const names = Object.keys(declarations ?? {});
  const canExpand = wide && document.fullscreenEnabled;
  const followWithState = (message: string) =>
    follow?.(
      [message, ...(modelContent.current == null ? [] : [JSON.stringify(modelContent.current)])].join('\n'),
    );
  return (
    <div className="visual-answer visual-codex" data-openbitfun-component="generative-widget" data-openbitfun-part="application" ref={wrapper} data-figure-check={verified ? 'passed' : issue || loadError ? 'failed' : 'pending'} data-wide={wide || undefined} data-codex-figure>
      {verified && (canExpand || visible.length > 0 || layout || dashboard || names.length > 0) && (
        <details className="visual-codex__menu" data-openbitfun-component="generative-widget" data-openbitfun-part="toolbar">
          <summary aria-label={t('visualize.options')} title={t('visualize.options')}>
            ···
          </summary>
          <div
            className="visual-answer__controls"
            onClick={(e) => e.currentTarget.closest('details')?.removeAttribute('open')}
          >
            {names.length > 0 && (
              <button
                type="button"
                aria-expanded={sourcesOpen}
                onClick={() => {
                  setSelectedSources(null);
                  setSourcesOpen(!sourcesOpen);
                }}
              >
                {t('visualize.sources')}
              </button>
            )}
            {canExpand && (
              <button
                type="button"
                aria-expanded={expanded}
                onClick={() => {
                  const change = expanded ? document.exitFullscreen() : wrapper.current?.requestFullscreen();
                  change?.catch(() => setAction({ title: t('visualize.expandFailed') }));
                }}
              >
                {t(expanded ? 'visualize.collapse' : 'visualize.expand')}
              </button>
            )}
            {(visible.length > 0 || layout) && (
              <button
                type="button"
                aria-expanded={controlsOpen}
                onClick={() => {
                  setControlsOpen(!controlsOpen);
                  setAnnotating(false);
                }}
              >
                {t('visualize.adjust')}
              </button>
            )}
            {dashboard && follow && (
              <button
                type="button"
                aria-pressed={annotating}
                onClick={() => {
                  setAnnotating(!annotating);
                  setControlsOpen(false);
                }}
              >
                {t('visualize.annotate')}
              </button>
            )}
          </div>
        </details>
      )}
      <iframe
        data-openbitfun-component="generative-widget" data-openbitfun-part="app"
        key={built?.version}
        title={title}
        ref={ref}
        sandbox="allow-scripts"
        referrerPolicy="no-referrer"
        allow="camera 'none'; microphone 'none'; geolocation 'none'; clipboard-read 'none'; clipboard-write 'none'"
        aria-hidden={!verified || undefined}
        tabIndex={verified ? undefined : -1}
        style={{ height, colorScheme: scheme, ...(verified ? {} : { position: 'absolute', opacity: 0, pointerEvents: 'none' }) }}
      />
      {annotating && annotation && (
        <label>
          {annotation.label ?? annotation.id}
          <textarea value={note} onChange={(e) => setNote(e.target.value)} maxLength={4000} />
          <button
            type="button"
            disabled={!note.trim()}
            onClick={() => {
              followWithState([title, annotation.label ?? annotation.id, note.trim()].join('\n'));
              setAnnotation(null);
              setAnnotating(false);
              setNote('');
            }}
          >
            {t('visualize.followUp')}
          </button>
        </label>
      )}
      {controlsOpen &&
        visible.map((group) => (
          <fieldset key={group.registrationId}>
            <legend>{group.targets[0].label}</legend>
            {group.annotationControls.controls.map((c) => {
              const value = edits[c.callback] ?? c.currentValue;
              const change = (v: Value) => setEdits((prev) => ({ ...prev, [c.callback]: v }));
              return (
                <label key={c.callback}>
                  {c.label}
                  {c.type === 'select' ? (
                    <select value={String(value)} onChange={(e) => change(e.target.value)}>
                      {c.options.map((o) => (
                        <option key={o.value} value={o.value}>
                          {o.label}
                        </option>
                      ))}
                    </select>
                  ) : c.type === 'toggle' ? (
                    <input
                      type="checkbox"
                      checked={value === true}
                      onChange={(e) => change(e.target.checked)}
                    />
                  ) : c.type === 'range' ? (
                    <input
                      type="range"
                      min={c.min}
                      max={c.max}
                      step={c.step}
                      value={Number(value)}
                      onChange={(e) => change(e.target.valueAsNumber)}
                    />
                  ) : (
                    <input type="color" value={String(value)} onChange={(e) => change(e.target.value)} />
                  )}
                </label>
              );
            })}
          </fieldset>
        ))}
      {controlsOpen && Object.keys(edits).length > 0 && (
        <div className="visual-answer__controls">
          <button type="button" aria-pressed={original} onClick={() => setOriginal(!original)}>
            {t('visualize.previewOriginal')}
          </button>
          <button
            type="button"
            onClick={() => {
              setEdits({});
              setOriginal(false);
            }}
          >
            {t('visualize.resetAdjustments')}
          </button>
          {follow && (
            <button
              type="button"
              onClick={() => {
                const lines = groups.flatMap((g) =>
                  g.annotationControls.controls
                    .filter((c) => Object.prototype.hasOwnProperty.call(edits, c.callback))
                    .map((c) => g.targets[0].label + ' · ' + c.label + ' = ' + edits[c.callback]),
                );
                followWithState([title, ...lines].join('\n'));
              }}
            >
              {t('visualize.followUp')}
            </button>
          )}
        </div>
      )}
      {action && (
        <div className="visual-answer__controls">
          {action.href ? (
            <a target="_blank" rel="noopener noreferrer" href={action.href}>
              {action.href}
            </a>
          ) : !action.prompt ? (
            <span>{action.title}</span>
          ) : (
            <>
              <span>{action.prompt}</span>
              <button
                type="button"
                onClick={() => {
                  if (action.prompt) followWithState(action.prompt);
                  setAction(null);
                }}
              >
                {action.title ?? t('visualize.followUp')}
              </button>
            </>
          )}
          <button type="button" onClick={() => setAction(null)}>
            {t('visualize.dismiss')}
          </button>
        </div>
      )}
      {!declarations && <small role="status">{t('visualize.invalidBindings')}</small>}
      {names.some((n) => !(bindings[n] || liveValues[n])) && (
        <small role="status">
          {t('visualize.missingSources', {
            names: names.filter((n) => !(bindings[n] || liveValues[n])).join(', '),
          })}
        </small>
      )}
      {names.length > 0 && declarations && (
        <section hidden={!sourcesOpen} aria-label={t('visualize.sources')}>
          {sourceLabels.length > 0 && <p>{sourceLabels.map((tab) => tab.label).join(' · ')}</p>}
          {names.map((name) => {
            const binding = declarations[name];
            const metadata = sourceMetadata.find((entry) => entry.name === name);
            const refresh = binding.type === 'tool' || binding.type === 'library';
            return (
              <details key={name} hidden={!!selectedSources && !selectedSources.includes(name)} open>
                <summary>{metadata?.title || name}</summary>
                {metadata?.description && <p>{metadata.description}</p>}
                <pre>{JSON.stringify(binding, null, 2)}</pre>
                {follow && refresh && getSession() && (
                  <LiveResourceBinding
                    session={getSession()!}
                    format={binding.type === 'tool' ? 'json' : 'file'}
                    interval={binding.refreshIntervalSeconds}
                    onData={(value) => setLiveValues((previous) => ({ ...previous, [name]: value }))}
                  />
                )}
                {follow && refresh && (
                  <label>
                    <input
                      type="checkbox"
                      checked={watching.includes(name)}
                      onChange={(e) =>
                        setWatching((previous) =>
                          e.target.checked ? [...previous, name] : previous.filter((n) => n !== name),
                        )
                      }
                    />
                    {t('visualize.watchSession', { seconds: binding.refreshIntervalSeconds })}
                  </label>
                )}
                {follow && refresh && (
                  <button
                    type="button"
                    onClick={() => {
                      const request = '[visualize:' + crypto.randomUUID() + ']';
                      setRequests((previous) => ({ ...previous, [name]: request }));
                      follow(
                        t('visualize.refreshPrompt', { name, source: JSON.stringify(binding), request }),
                      );
                    }}
                  >
                    {t(requests[name] ? 'visualize.refreshPending' : 'visualize.refreshSource')}
                  </button>
                )}
              </details>
            );
          })}
        </section>
      )}
      {mapNeedsConsent && (
        <div role="status">
          {t('visualize.deferredStreetMap')}{' '}
          <button type="button" onClick={() => setOnlineRevision(html)}>{t('visualize.loadStreetMap')}</button>
        </div>
      )}
      {!mapNeedsConsent && !verified && !loadError && !issue && <small role="status">{t('visualize.loading')}</small>}
      {(loadError || issue) && (
        <div role="status">
          {t('visualize.resourceError')} {issue}{' '}
          <button type="button" onClick={() => setAttempt((n) => n + 1)}>
            {t('visualize.retry')}
          </button>
        </div>
      )}
    </div>
  );
}
