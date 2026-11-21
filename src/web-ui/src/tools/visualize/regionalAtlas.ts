import atlasStyle from '../../../../shared/visualize/regional-map.css?raw';
import { i18nService } from '@/infrastructure/i18n';

import land from '../../../../../third_party/visualize/atlas/land.json?url&no-inline';
import rivers from '../../../../../third_party/visualize/atlas/rivers.json?url&no-inline';
import lakes from '../../../../../third_party/visualize/atlas/lakes.json?url&no-inline';
import places from '../../../../../third_party/visualize/atlas/places.json?url&no-inline';
import borders from '../../../../../third_party/visualize/atlas/borders.json?url&no-inline';
import provinces from '../../../../../third_party/visualize/atlas/provinces.json?url&no-inline';
import seas from '../../../../../third_party/visualize/atlas/seas.json?url&no-inline';

const layers = { land, rivers, lakes, places, borders, provinces, seas };

let placeNames: Promise<Set<string>> | undefined;

/** An unqualified "map of X" needs positive geographic evidence before it can
 * invalidate an otherwise legitimate figure. Reuse the shipped gazetteer. */
export async function isAtlasPlace(name: string) {
  placeNames ??= fetch(places).then(async response => {
    if (!response.ok) throw Error('Regional place names unavailable');
    const rows: [string, string, string[]][] = await response.json();
    const names = new Set<string>();
    const countries = new Set<string>();
    for (const [country, region, aliases] of rows) {
      for (const alias of aliases) names.add(alias.toLowerCase());
      if (region) names.add(region.toLowerCase());
      if (/^[A-Z]{2}$/.test(country)) countries.add(country);
    }
    const english = new Intl.DisplayNames(['en'], { type: 'region' });
    for (const code of countries) names.add((english.of(code) ?? code).toLowerCase());
    return names;
  }).catch(error => {
    placeNames = undefined;
    throw error;
  });
  return (await placeNames).has(name.toLowerCase());
}

type AtlasRequest = { detail?: string; places?: (string | { coordinates?: unknown } | null)[] };

function needsPlaceLookup(spec: AtlasRequest) {
  return spec?.detail === 'street' && Array.isArray(spec.places) &&
    spec.places.some(place => typeof place === 'string' || !place?.coordinates);
}

async function project(spec: AtlasRequest, language: string): Promise<string> {
  const urls = Object.fromEntries(Object.entries(layers).map(([key, url]) => [key, new URL(url, location.href).href]));
  const worker = new Worker(new URL('./regionalProjection.js', import.meta.url), { type: 'module' });
  try {
    return await new Promise<string>((resolve, reject) => {
      // Named stops may require a cold geocoding connection before the tile requests.
      const workerBudget = needsPlaceLookup(spec) ? 60000 : 15000;
      const deadline = Date.now() + workerBudget;
      const timeout = window.setTimeout(() => reject(Error('Map data did not load in time. For an unavailable street map, show a clearly labeled stop list without a scale.')), workerBudget);
      worker.onmessage = ({ data }) => {
        clearTimeout(timeout);
        if (typeof data.svg === 'string') resolve(data.svg);
        else reject(Error(data.issue || 'Map projection failed.'));
      };
      worker.onerror = (e) => {
        clearTimeout(timeout);
        reject(Error(e.message || 'The regional map worker could not load.'));
      };
      const options = { lng: language };
      const labels = {
        north: i18nService.t('flow-chat:visualize.mapNorth', options),
        km: i18nService.t('flow-chat:visualize.mapKilometers', options),
        m: i18nService.t('flow-chat:visualize.mapMeters', options),
      };
      worker.postMessage({ spec, urls, language, labels, deadline: deadline - 1000, lookupDeadline: deadline - 15000 });
    });
  } finally {
    worker.terminate();
  }
}

export async function materializeRegionalMaps(source: string, publishing = false, allowPublicLookup = true, onStage?: () => Promise<void>) {
  if (!source.includes('data-visual-atlas')) return source;
  const fragment = document.createElement('template');
  fragment.innerHTML = source;
  const maps = fragment.content.querySelectorAll('script[type="application/json"][data-visual-atlas="1"]');
  if (!maps.length) return source;
  for (const declaration of maps) {
    const spec = JSON.parse(declaration.textContent || 'null');
    if (spec?.detail === 'street' && !publishing)
      throw Error('This street map has no saved basemap. Publish it on the producing host before replay.');
    if (!allowPublicLookup && needsPlaceLookup(spec))
      throw Error('A figure with bound data cannot send stop names to a public geocoder. Supply sourced coordinates or local projected geometry.');
    const language = spec?.language || document.documentElement.lang || i18nService.getCurrentLocale();
    await onStage?.();
    const svg = await project(spec, language);
    const result = document.createElement('template');
    result.innerHTML = svg;
    const style = document.createElementNS('http://www.w3.org/2000/svg', 'style');
    style.textContent = atlasStyle;
    result.content.querySelector('svg')!.prepend(style);
    const credit = document.createElement('small');
    if (spec.detail === 'street') {
      const attribution = document.createElement('a');
      attribution.href = 'https://www.openstreetmap.org/copyright';
      attribution.textContent = '© OpenStreetMap contributors';
      credit.append(attribution, ' · VersaTiles');
    } else {
      credit.textContent = i18nService.t('flow-chat:visualize.mapCredit', { lng: language });
    }
    credit.className = 'text-muted';
    declaration.replaceWith(result.content, credit);
  }
  await onStage?.();
  const html = fragment.innerHTML;
  if (new TextEncoder().encode(html).length > 1024 * 1024)
    throw Error('The projected map exceeds the figure size limit. Use a regional extent.');
  return html;
}

// A headless producer may save only the declaration. Opening history must not
// disclose its places or coordinates from a different reader's network.
export async function recoverReaderStreetMap(source: string, ownerKey: string, allowNames: boolean, loadOnline: boolean): Promise<{ html: string; save?: () => void } | undefined> {
  const fragment = document.createElement('template');
  fragment.innerHTML = source;
  const declarations = Array.from(fragment.content.querySelectorAll('script[type="application/json"][data-visual-atlas="1"]'),
    el => JSON.parse(el.textContent || 'null'));
  if (!declarations.some(spec => spec?.detail === 'street')) return { html: source };
  if (!allowNames && declarations.some(needsPlaceLookup))
    throw Error('This bound street map requires sourced stop coordinates or a basemap saved by its producer.');
  const key = 'visualize.street-map:' + ownerKey;
  // Cache the generated markup under the original source revision. Readers
  // can still open the figure when persistent storage is unavailable.
  const digest = globalThis.crypto?.subtle && new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(source))).join('.');
  if (digest) {
    try {
      const saved = JSON.parse(localStorage.getItem(key) ?? 'null');
      if (saved?.revision === digest && typeof saved.html === 'string') return { html: saved.html };
    } catch { /* Missing or unreadable cache. */ }
  }
  if (!loadOnline) return undefined;
  const html = await materializeRegionalMaps(source, true, allowNames);
  // Save only after the actual frame passes the reader's render check.
  return { html, save: () => {
    if (!digest) return;
    try { localStorage.setItem(key, JSON.stringify({ revision: digest, html })); }
    catch { /* Keep displaying the current result when storage is full. */ }
  } };
}
