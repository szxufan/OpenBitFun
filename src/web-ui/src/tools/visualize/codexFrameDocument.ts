import type { BoundValues, Declarations } from './visualBindings';
import { snapshotFigureScripts } from './figureResources';
import { localizeFigureImages } from './figureImages';
import { cachedVisualRuntime } from './cachedVisualRuntime';
import builtInGuest from './figureGuest.js?raw';
import builtInControls from './figureDesignControls.js?raw';
import builtInStyles from './inlineDocument.css?raw';
import hostStyles from './figureHost.css?raw';
import chartTheme from './figureChartTheme.js?raw';
import extras from './codexFrameExtras.js?raw';
import inspection from './inspectRenderedFigure.js?raw';
import release from '../../../../../scripts/visualize-runtime.json';

// The kit is loaded only for v2 figures. One local import supplies both the
// runtime and the exact same authoring resources to the installed skill.
const raw = import.meta.env.OPENBITFUN_VISUALIZE_DEV_KIT ? import.meta.glob<string>(
  '../../../../crates/assembly/core/builtin_skills/visualize/upstream/{assets/*.js,skills/visualize/assets/*}',
  { eager: true, import: 'default', query: '?raw' },
) : {};
const assets = Object.fromEntries(
  Object.entries(raw).map(([file, source]) => [file.split('/').pop()!, source]),
);
const script = (source: string) => '<script>' + source.replace(/<\/script/gi, '<\\/script') + '</script>';
// Use the same isolated document for initial inspection, display and state restoration.
const offlineCsp = [
  "default-src 'none'", "base-uri 'none'", "form-action 'none'", "frame-src 'none'", "object-src 'none'",
  "script-src 'unsafe-inline' 'unsafe-eval' blob: data:", "style-src 'unsafe-inline' blob: data:",
  "connect-src blob: data:", "worker-src blob:", "img-src blob: data:", "font-src blob: data:",
  "media-src blob: data:", "webrtc 'block'",
].join('; ');

function offlineFrame(document: string) {
  const serialized = JSON.stringify(document).replace(/</g, '\\u003c');
  return '<!doctype html><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="' +
    offlineCsp + '"><meta http-equiv="x-dns-prefetch-control" content="off">' +
    '<style>html,body,iframe{width:100%;height:100%;margin:0;border:0;display:block;overflow:hidden}</style><body>' +
    script(`const frame=document.createElement('iframe');frame.setAttribute('sandbox','allow-scripts');
      frame.referrerPolicy='no-referrer';frame.title='Visualization';
      addEventListener('message',event=>{
        const target=event.source===parent?frame.contentWindow:event.source===frame.contentWindow?parent:null;
        if(target)target.postMessage(event.data,'*',event.ports);
      });
      frame.srcdoc=${serialized};document.body.append(frame);`);
}

const hasReferenceKit = !!assets['visualize.html'] && !!assets['standalone-host-bridge.js'];

export async function buildCodexFrame(
  fragment: string,
  state: unknown,
  lang: string,
  declarations: Declarations,
  values: BoundValues,
  figureStyle: Record<string, string>,
) {
  if (fragment.includes('data-visual-atlas')) {
    const { materializeRegionalMaps } = await import('./regionalAtlas');
    fragment = await materializeRegionalMaps(fragment);
  }
  // Use Tweak's existing hidden-attribute observer for tabs and conditional content as well as variants.
  const tweak = assets['tweak.js']?.split('.viz-carousel > [data-variant][hidden]').join('[hidden]') ?? builtInControls;
  let extensions = script(assets['standalone-host-bridge.js'] ?? builtInGuest) + script(extras) + script(inspection) + script(tweak);
  if (/<viz-calendar\b/i.test(fragment) && !/\/widgets\/[^/]+\/(?:calendar|element)\.js/.test(fragment)) {
    if (hasReferenceKit) extensions += script(assets['calendar.js']);
    else fragment = '<script type="module">import "' + release['calendar.js'].url + '"</script>' + fragment;
  }
  if (
    hasReferenceKit && /<(?:experimental-)?dash-[a-z-]+\b|<dash-sources\b|data-dashboard-|class=["'][^"']*dashboard-/.test(
      fragment,
    )
  ) {
    for (const name of [
      'dashboard-sources.js',
      'dashboard-layout.js',
      'dashboard-source-actions.js',
      'dashboard-annotations.js',
    ]) {
      extensions += script(assets[name]);
    }
  }
  if (/\becharts\s*\./.test(fragment)) {
    const { default: charts } = await import('echarts/dist/echarts.min.js?raw');
    extensions += script(charts) + script(chartTheme);
  }
  values = await localizeFigureImages(declarations, values);
  const guestDeclarations = Object.fromEntries(Object.entries(declarations).map(([name, binding]) =>
    [name, binding.type === 'image' ? { type: 'image', url: values[name].value } : binding]));
  const bindings = JSON.stringify({ declarations: guestDeclarations, values, figureStyle }).replace(/</g, '\\u003c');
  const initial = JSON.stringify({ widgetState: state, statePersistence: 'local' }).replace(/</g, '\\u003c');
  const cache = await cachedVisualRuntime(await snapshotFigureScripts(fragment, declarations));
  let kit = (assets['visualize.html'] ?? '<!--__INLINE_VISUALIZATION_FRAGMENT__-->').replace(
    '<!--__INLINE_VISUALIZATION_FRAGMENT__-->',
    () => extensions + cache.fragment,
  );
  if (cache.lucide) kit = kit.replace(/<script id="codex-visualization-lucide"[^>]*><\/script>/, '');
  const document = (
    '<!doctype html><html lang="' +
    (/^[a-z\d-]+$/i.test(lang) ? lang : 'en') +
    '"><head>' +
    '<meta charset="utf-8"><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="' +
    offlineCsp +
    '">' +
    '<meta name="viewport" content="width=device-width, initial-scale=1"><style>' +
    (assets['visualize.css'] ?? builtInStyles) +
    '\n:root{--control-spacing:8px}.btn,.nav-link{border-radius:8px}html>body{padding:0;margin:0}' +
    hostStyles +
    // Honor the fragment's hidden state even when its layout uses flex or grid.
    '[hidden]:not([hidden="until-found" i]){display:none!important}</style></head><body>' +
    '<script type="application/json" id="codex-visualization-widget-state">' +
    initial +
    '</script>' +
    '<script type="application/json" id="openbitfun-visual-data">' +
    bindings +
    '</script>' +
    cache.scripts +
    kit +
    '</body></html>'
  );
  return offlineFrame(document);
}
