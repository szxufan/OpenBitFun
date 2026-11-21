import release from '../../../../../scripts/visualize-runtime.json';

const loaders = import.meta.env.OPENBITFUN_VISUALIZE_DEV_KIT ? import.meta.glob<string>('../../../../../.visualize-runtime/*.{js,json}', {
  query: '?raw',
  import: 'default',
}) : {};
const bundled = import.meta.glob<string>('../../../../../third_party/visualize/runtime/*.{js,json}', {
  query: '?raw', import: 'default',
});
const modules = new Map(Object.entries({ ...loaders, ...bundled }).map(([path, load]) => [path.split('/').pop()!, load]));
type Asset = keyof typeof release;
const names = Object.keys(release) as Asset[];
const inline = (body: string) => '<script>' + body.replace(/<\/script/gi, '<\\/script') + '</script>';

// Module identity is observable through import.meta.url. Keep each verified
// release URL as its logical identity even when its bytes are served offline.
export async function cachedVisualRuntime(fragment: string) {
  const selected = new Set<Asset>(['lucide.js', 'floating-core.js', 'floating-dom.js']);
  const chartReleases = [
    ['chart-v1.8.1.js', 'chart-element-v1.8.1.js', 'chart-runtime-v1.8.1.js'],
    ['chart.js', 'chart-element.js', 'chart-runtime.js'],
  ] as const;
  for (const [entry, element, runtime] of chartReleases) {
    if (fragment.includes(release[entry].url) || fragment.includes(release[element].url)) {
      selected.add(entry);
      selected.add(element);
      selected.add(runtime);
    }
  }
  for (const name of names) if (fragment.includes(release[name].url)) selected.add(name);
  const urls = fragment.match(/https:\/\/[^\s"'<>\x60]+/g) ?? [];
  const aliases: Record<string, string[]> = {};
  function match(key: Asset, pattern: RegExp) {
    aliases[key] = urls.filter((url) => pattern.test(url));
    if (aliases[key].length) selected.add(key);
  }
  match(
    'd3-geo-3.1.0.js',
    /^https:\/\/(?:esm\.sh\/d3-geo@3\.1\.0|cdn\.jsdelivr\.net\/npm\/d3-geo@3\.1\.0\/\+esm)$/,
  );
  match('countries.json', /^https:\/\/esm\.sh\/@d3-maps\/atlas@1\.0\.0\/world\/countries\/countries-110m$/);
  match(
    'counties.json',
    /^https:\/\/cdn\.jsdelivr\.net\/npm\/us-atlas@3(?:\.0\.1)?\/counties-10m\.json\/\+esm$/,
  );
  match(
    'd3.js',
    /^https:\/\/(?:esm\.sh\/|cdn\.jsdelivr\.net\/npm\/)(?:d3-geo@3(?:\.1\.1)?|d3@7(?:\.9\.0)?)(?:\/\+esm)?$/,
  );
  match(
    'topojson.js',
    /^https:\/\/(?:esm\.sh\/|cdn\.jsdelivr\.net\/npm\/)topojson-client@3(?:\.1\.0)?(?:\/\+esm)?$/,
  );
  // Classic script tags do not use import maps. Preserve their parser ordering,
  // onload handlers and integrity checks while serving the exact pinned bytes.
  const classic = names.filter(key => key.endsWith('.umd.js') || ['lucide.js', 'floating-core.js', 'floating-dom.js', 'd3.js', 'topojson.js'].includes(key));
  let template: HTMLTemplateElement | undefined;
  const classicTags = new Map<HTMLScriptElement, Asset>();
  if (/<script\s[^>]*src\s*=/i.test(fragment)) {
    template = document.createElement('template');
    template.innerHTML = fragment;
    for (const tag of template.content.querySelectorAll<HTMLScriptElement>('script[src]')) {
      if (tag.type === 'module') continue;
      const key = classic.find(name => release[name].url === tag.getAttribute('src'));
      if (key) {
        classicTags.set(tag, key);
        selected.add(key);
      }
    }
  }
  const sources = new Map<Asset, string>();
  await Promise.all(
    [...selected].map(async (name) => {
      const load = modules.get(name);
      if (load) {
        sources.set(name, await load());
      } else if (release[name].url.startsWith('https://persistent.oaistatic.com/plugins/visualize/')) {
        // Only the host can load a missing pinned release. Fetch verifies the
        // manifest digest before the bytes enter the offline guest's import map.
        const asset = release[name];
        const integrity = 'sha256-' + btoa(String.fromCharCode(...asset.sha256.match(/../g)!.map(hex => parseInt(hex, 16))));
        const response = await fetch(asset.url, {
          integrity, credentials: 'omit', mode: 'cors', redirect: 'error', referrerPolicy: 'no-referrer',
          signal: AbortSignal.timeout(4000),
        });
        if (!response.ok) throw Error('Packaged visualization dependency unavailable: ' + name);
        sources.set(name, await response.text());
      }
    }),
  );
  let scripts = '';
  const esm: [string[], string][] = [];
  for (const key of selected) {
    let code = sources.get(key);
    if (!code) throw Error('Missing packaged visualization runtime: ' + key);
    if (key.endsWith('.umd.js')) continue;
    const url = release[key].url;
    if (['lucide.js', 'floating-core.js', 'floating-dom.js', 'd3.js', 'topojson.js'].includes(key)) {
      scripts += inline(code);
      if (key === 'd3.js' || key === 'topojson.js') {
        const lib = key === 'd3.js' ? 'd3' : 'topojson';
        scripts += inline(
          'globalThis.__visualExports ??= {};globalThis.__visualExports.' +
            lib +
            '=Object.keys(globalThis.' +
            lib +
            ').filter(k=>/^[A-Za-z_$][\\w$]*$/.test(k));',
        );
        esm.push([aliases[key] ?? [], '@exports:' + lib]);
      }
    } else {
      if (chartReleases.some(([entry]) => entry === key))
        code = code.replace(/"\.\.\/[^"]+"/g, (path) => JSON.stringify(new URL(JSON.parse(path), url).href));
      if (key.endsWith('.json')) code = 'export default ' + code;
      else code = code.replace(/import\.meta\.url/g, () => JSON.stringify(url));
      esm.push([[url, ...(aliases[key] ?? [])], code]);
    }
  }
  if (esm.length) {
    // Blob URLs must be created in the opaque guest origin to work in WebKit.
    scripts += inline(
      '(()=>{const imports=Object.create(null);for(let [ids,code] of ' +
        JSON.stringify(esm) +
        '){' +
        'if(code.startsWith("@exports:")){const lib=code.slice(9);code="export const {"+globalThis.__visualExports[lib].join(",")+"}=globalThis."+lib+";export default globalThis."+lib;}' +
        'const url=URL.createObjectURL(new Blob([code],{type:"text/javascript"}));for(const id of ids)imports[id]=url;}' +
        'const map=document.createElement("script");map.type="importmap";map.textContent=JSON.stringify({imports});document.head.append(map);})();',
    );
  }
  for (const [tag, key] of classicTags) {
    // A percent-encoded data URL preserves UTF-8 and the original script bytes.
    tag.src = 'data:text/javascript;charset=utf-8,' + encodeURIComponent(sources.get(key)!);
  }
  return { scripts, fragment: classicTags.size ? template!.innerHTML : fragment, lucide: sources.has('lucide.js') };
}
