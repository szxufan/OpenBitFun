import { afterEach, expect, it, vi } from 'vitest';
import { resolveStreetStops } from './streetPlaces.js';

const city = ['CN', 'Zhejiang', ['Hangzhou', '杭州', '杭州市'], [120.168, 30.252]];
const spec = { city: '杭州', country: 'CN' };
function feature(id, tag, xy = [120.159, 30.257], cityName = '杭州市') {
  const [osm_key, osm_value] = tag.split(':');
  return { geometry: { type: 'Point', coordinates: xy }, properties: {
    name: '龙翔桥', osm_type: 'N', osm_id: id, osm_key, osm_value, city: cityName, countrycode: 'cn',
  } };
}
function serve(features) {
  const fetch = vi.fn(async () => Response.json({ features }));
  vi.stubGlobal('fetch', fetch);
  return fetch;
}
afterEach(() => vi.unstubAllGlobals());

it('recovers a named stop after a dropped connection or a temporary upstream failure', async () => {
  for (const transient of [new TypeError('Load failed'), new Response(null, { status: 503 })]) {
    const fetch = serve([feature(17, 'railway:station')]);
    if (transient instanceof Error) fetch.mockRejectedValueOnce(transient);
    else fetch.mockResolvedValueOnce(transient);
    const [stop] = await resolveStreetStops([{ name: '龙翔桥' }], spec, [city]);
    expect(stop.osmId).toBe('N17');
    expect(fetch).toHaveBeenCalledTimes(2);
  }
});

it('preserves the failed query and HTTP or decoding reason without retrying permanent responses', async () => {
  for (const [response, reason] of [
    [new Response(null, { status: 429 }), 'Photon returned HTTP 429 (rate limited)'],
    [new Response(null, { status: 403 }), 'Photon returned HTTP 403'],
    [new Response(null, { status: 503, headers: { 'retry-after': '120' } }), 'Photon returned HTTP 503'],
    [new Response('not JSON'), 'Photon returned an invalid place response'],
  ]) {
    const fetch = vi.fn().mockResolvedValue(response);
    vi.stubGlobal('fetch', fetch);
    await expect(resolveStreetStops([{ name: '西湖', query: '涌金公园' }], spec, [city]))
      .rejects.toThrow('lookup unavailable for "涌金公园": ' + reason);
    expect(fetch).toHaveBeenCalledTimes(1);
  }
});

it('keeps verified locations available for repair when another public place cannot be queried', async () => {
  vi.stubGlobal('fetch', async url => url.searchParams.get('q') === '龙翔桥'
    ? Response.json({ features: [feature(17, 'railway:station')] })
    : new Response(null, { status: 503 }));
  await expect(resolveStreetStops([{ name: '龙翔桥' }, { name: 'Museum' }], spec, [city]))
    .rejects.toThrow(/Confirmed: 龙翔桥 \[120\.159,30\.257\].*Museum.*HTTP 503/);
});

it('resolves the requested object within its city and retains supplied points and stop order', async () => {
  const station = feature(17, 'railway:station');
  const fetch = serve([station, station, feature(18, 'highway:bus_stop')]);
  const first = { name: 'First stop', coordinates: [120.15, 30.24] };
  const stop = { name: '龙翔桥地铁站', query: '龙翔桥', osmTag: 'railway:station' };
  const result = await resolveStreetStops([first, stop, stop], spec, [city]);
  expect(result).toEqual([first, ...[stop, stop].map(s => ({ ...s, coordinates: [120.159, 30.257], osmId: 'N17', osmName: '龙翔桥' }))]);
  expect(fetch).toHaveBeenCalledTimes(1);
  const [[url, options]] = fetch.mock.calls;
  expect(url.origin).toBe('https://photon.komoot.io');
  expect(url.searchParams.get('q')).toBe('龙翔桥');
  expect(url.searchParams.get('osm_tag')).toBe('railway:station');
  expect(url.searchParams.has('bbox')).toBe(true);
  expect(options).toMatchObject({ credentials: 'omit', redirect: 'error', referrerPolicy: 'no-referrer' });
});

it('reports ambiguous and missing stops together and accepts an explicitly selected candidate', async () => {
  serve([feature(17, 'railway:station'), feature(18, 'highway:bus_stop')]);
  await expect(resolveStreetStops([{ name: '龙翔桥' }, { name: 'Missing' }], spec, [city]))
    .rejects.toThrow(/龙翔桥.*ambiguous.*N17.*N18.*Missing.*no exact match/);
  const [selected] = await resolveStreetStops([{ name: 'Transit stop', query: 'Local name alias', osmId: 'n18' }], spec, [city]);
  expect(selected.osmId).toBe('N18');
});

it('rejects nearby homonyms from another city, distant suburbs and a truncated result set', async () => {
  serve([feature(1, 'railway:station', [120.159, 30.257], 'Another City'), feature(2, 'railway:station', [119.77, 30.26])]);
  await expect(resolveStreetStops([{ name: '龙翔桥' }], spec, [city])).rejects.toThrow('no exact match');
  serve(Array(16).fill(feature(1, 'railway:station')));
  await expect(resolveStreetStops([{ name: '龙翔桥' }], spec, [city])).rejects.toThrow('ambiguous');
});

it('only reports confirmed coordinates when an authored OSM id is missing', async () => {
  serve([feature(17, 'railway:station')]);
  const places = [{ name: 'Verified', query: '龙翔桥', osmId: 'N17' }, { name: 'Unresolved', osmId: 'N99' }];
  await expect(resolveStreetStops(places, spec, [city]))
    .rejects.toThrow(/Confirmed: Verified \[120\.159,30\.257\]\. Unresolved \(no exact match\)/);
});

it('suggests a station by its display name when the expanded query misses, without guessing the stop', async () => {
  const fetch = vi.fn(async url => Response.json({
    features: url.searchParams.get('q') === '龙翔桥' ? [feature(17, 'railway:station')] : [],
  }));
  vi.stubGlobal('fetch', fetch);
  const stop = { name: '龙翔桥', query: '龙翔桥 地铁站', osmTag: 'railway:station' };
  await expect(resolveStreetStops([stop], spec, [city])).rejects.toThrow(/no exact match.*N17.*railway:station/);
  const [confirmed] = await resolveStreetStops([{ ...stop, osmId: 'n17' }], spec, [city]);
  expect(confirmed.osmId).toBe('N17');
});

it('includes a pedestrian segment in repair hints when the first road segments all have the same type', async () => {
  serve([1, 2, 3, 4, 5].map(id => feature(id, 'highway:tertiary')).concat(feature(6, 'highway:pedestrian')));
  await expect(resolveStreetStops([{ name: '龙翔桥' }], spec, [city])).rejects.toThrow(/N1.*N6.*highway:pedestrian/);
});

it('requires an identified city before going online and never substitutes its center when offline', async () => {
  const fetch = vi.fn().mockRejectedValue(Error('Offline'));
  vi.stubGlobal('fetch', fetch);
  await expect(resolveStreetStops([{ name: '龙翔桥' }], {}, [city])).rejects.toThrow('set city');
  expect(fetch).not.toHaveBeenCalled();
  await expect(resolveStreetStops([{ name: '龙翔桥' }], spec, [city])).rejects.toThrow('Photon network request failed (Offline)');
  expect(fetch).toHaveBeenCalledTimes(2);
});

it('returns confirmed coordinates and ambiguous candidates when another lookup exhausts the shared deadline', async () => {
  vi.useFakeTimers();
  try {
    const fetch = vi.fn(async (url, options) => {
      const q = url.searchParams.get('q');
      if (q === 'Slow') return new Promise((_, reject) => {
        options.signal.addEventListener('abort', () => reject(Error('aborted')), { once: true });
      });
      return Response.json({ features: q === '龙翔桥' ? [feature(17, 'railway:station')] :
        [feature(81, 'tourism:attraction'), feature(82, 'tourism:museum')] });
    });
    vi.stubGlobal('fetch', fetch);
    const result = resolveStreetStops([{ name: '龙翔桥' }, { name: 'Ambiguous' }, { name: 'Slow' }], spec, [city], Date.now() + 11000)
      .catch(error => error.message);
    await vi.advanceTimersByTimeAsync(11000);
    const message = await result;
    expect(message).toContain('Confirmed: 龙翔桥 [120.159,30.257]');
    expect(message).toContain('N81');
    expect(message).toContain('Slow');
    expect(fetch).toHaveBeenCalledTimes(4);
  } finally { vi.useRealTimers(); }
});
