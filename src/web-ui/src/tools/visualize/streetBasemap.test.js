import { afterEach, expect, it, vi } from 'vitest';
import { geoMercator } from 'd3-geo';
import { PbfWriter } from 'pbf';
import { streetBasemap } from './streetBasemap.js';

const projection = geoMercator().center([120, 30]).scale(240000).translate([360, 240]);

// Minimal MVT containing a real linestring command sequence in the streets layer.
function streetTile() {
  const pbf = new PbfWriter();
  pbf.writeMessage(3, (_, layer) => {
    layer.writeVarintField(15, 2);
    layer.writeStringField(1, 'streets');
    layer.writeMessage(2, (_, feature) => {
      feature.writeVarintField(3, 2);
      feature.writePackedVarint(4, [9, 0, 0, 10, 8192, 8192]);
    });
  });
  return pbf.finish();
}

afterEach(() => vi.unstubAllGlobals());

it('projects only fixed public tile requests and emits offline street geometry', async () => {
  const fetch = vi.fn(async () => new Response(streetTile()));
  vi.stubGlobal('fetch', fetch);
  const svg = await streetBasemap(projection, 720, 480, 'en', []);
  expect(svg).toContain('data-street-layer="streets" d="M');
  expect(svg).not.toContain('https://');
  for (const [url, options] of fetch.mock.calls) {
    expect(url).toMatch(/^https:\/\/tiles\.versatiles\.org\/tiles\/osm-vector\/\d+\/\d+\/\d+$/);
    expect(options).toMatchObject({ credentials: 'omit', referrerPolicy: 'no-referrer', redirect: 'error' });
  }
});

it('uses the highest available source level for a close-up walking route', async () => {
  const closeUp = geoMercator().center([120.15, 30.26]).scale(2100000).translate([360, 240]);
  const fetch = vi.fn(async url => new Response(url.includes('/14/') ? streetTile() : null,
    { status: url.includes('/14/') ? 200 : 404 }));
  vi.stubGlobal('fetch', fetch);

  await expect(streetBasemap(closeUp, 720, 480, 'zh', [])).resolves.toContain('data-street-layer="streets"');
  expect(fetch.mock.calls.every(([url]) => url.includes('/14/'))).toBe(true);
});

it('rejects an incomplete viewport when one street tile is offline', async () => {
  let unavailableTile;
  const fetch = vi.fn(async url => {
    unavailableTile ??= url;
    if (url === unavailableTile) throw new Error('Offline');
    return new Response(streetTile());
  });
  vi.stubGlobal('fetch', fetch);
  await expect(streetBasemap(projection, 720, 480, 'en', [])).rejects.toThrow('Street basemap unavailable');
});

it('recovers a map after a transient failure without refetching the whole viewport', async () => {
  const fetch = vi.fn(async () => new Response(streetTile()));
  fetch.mockResolvedValueOnce(new Response(null, { status: 503 }));
  vi.stubGlobal('fetch', fetch);
  await expect(streetBasemap(projection, 720, 480, 'en', [])).resolves.toContain('data-street-layer="streets"');
  const urls = fetch.mock.calls.map(([url]) => url);
  expect(urls.length).toBe(new Set(urls).size + 1);
});
