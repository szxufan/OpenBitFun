import * as d3 from 'd3-geo';
import * as topojson from 'topojson-client';
import { streetBasemap } from './streetBasemap.js';
import { resolveStreetStops } from './streetPlaces.js';
self.onmessage = async ({ data: { spec, urls, language, labels: strings, lookupDeadline, deadline } }) => {
  const esc = (s) =>
    String(s).replace(
      /[&<>"']/g,
      (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c],
    );
  const load = async (key) => {
    const r = await fetch(urls[key]);
    if (!r.ok) throw Error('Offline regional map layer unavailable: ' + key);
    return r.json();
  };
  try {
    const { title, bounds } = spec;
    if (!Array.isArray(spec.places) || !spec.places.length || spec.places.length > 64) throw Error('Provide 1–64 places.');
    let places = spec.places.map(p => typeof p === 'string' ? { name: p } : p);
    if (places.some(p => !p || typeof p.name !== 'string' || !p.name.trim())) throw Error('A map marker needs a name.');
    const street = spec.detail === 'street';
    if (spec.detail !== undefined && !street && spec.detail !== 'regional')
      throw Error('Unknown map detail. Use street or regional.');
    const needsCoordinates = places.some(place => !place.coordinates);
    const gazetteer = needsCoordinates ? await load('places') : [];
    if (street && needsCoordinates) places = await resolveStreetStops(places, spec, gazetteer, lookupDeadline);
    const points = places.map((place) => {
      let xy = place.coordinates;
      if (!xy) {
        const match = (s) => s.toLowerCase() === (place.city || place.name).toLowerCase();
        let rows = gazetteer.filter(
          ([country, region, names]) =>
            names.some(match) &&
            (!place.country || country === place.country) &&
            (!place.region || region === place.region),
        );
        if (rows.length > 1) {
          const exact = rows.filter((r) => r[2].includes(place.name));
          if (exact.length === 1) rows = exact;
        }
        if (rows.length !== 1)
          throw Error(
            'Place lookup for "' +
              place.name +
              '" matched ' +
              rows.length +
              ' records. Specify country or region for cities. For sights and streets use detail:"street", city and the public stop names.',
          );
        xy = rows[0][3];
      }
      if (
        !Array.isArray(xy) ||
        xy.length !== 2 ||
        !xy.every(Number.isFinite) ||
        Math.abs(xy[0]) > 180 ||
        Math.abs(xy[1]) > 85
      )
        throw Error('Invalid longitude/latitude for ' + place.name);
      return { name: place.name, xy, osmId: street ? place.osmId : undefined, osmName: place.osmName, osmTag: place.osmTag };
    });
    const width = 720,
      height = 480,
      coordinates = points.map((p) => p.xy);
    let region = d3.geoBounds({ type: 'MultiPoint', coordinates });
    if (bounds !== undefined) {
      if (
        !Array.isArray(bounds) ||
        bounds.length !== 4 ||
        !bounds.every(Number.isFinite) ||
        Math.abs(bounds[0]) > 180 ||
        Math.abs(bounds[2]) > 180 ||
        bounds[1] < -85 ||
        bounds[3] > 85 ||
        bounds[3] <= bounds[1] ||
        bounds[0] === bounds[2]
      )
        throw Error('Invalid regional map bounds.');
      region = [bounds.slice(0, 2), bounds.slice(2)];
    }
    const [[west, south], [east, north]] = region;
    const dx = (east - west + 360) % 360;
    const midLon = ((west + dx / 2 + 540) % 360) - 180;
    const projection = d3.geoMercator().rotate([-midLon, 0]).scale(1).translate([0, 0]);
    const y0 = projection([midLon, north])[1],
      y1 = projection([midLon, south])[1];
    const scale = Math.min(
      (width * (bounds ? 1 : street ? 0.77 : 0.46)) / Math.max(street ? 0.00012 : 0.013, (dx * Math.PI) / 180),
      (height * (bounds ? 1 : street ? 0.72 : 0.41)) / Math.max(street ? 0.00008 : 0.009, y1 - y0),
    );
    projection
      .scale(scale)
      .translate([width / 2, height / 2 - ((y0 + y1) * scale) / 2])
      .clipExtent([
        [0, 0],
        [width, height],
      ]);
    const path = d3.geoPath(projection).digits(1);
    const languageKey = /^zh-(?:TW|HK|Hant)/i.test(language)
      ? 'name_zht'
      : /^zh/i.test(language)
        ? 'name_zh'
        : 'name';
    const terrainLabels = [];
    // Quantized TopoJSON can contain collapsed rings. With D3's spherical
    // winding, a tiny reversed/degenerate lake can otherwise paint the whole map.
    function polygon(rings) {
      const cleaned = [];
      for (const [i, ring] of rings.entries()) {
        if (ring.length < 4) {
          if (!i) return null;
          continue;
        }
        const area = d3.geoArea({ type: 'Polygon', coordinates: [ring] });
        const minor = Math.min(area, 4 * Math.PI - area);
        if (minor < 1e-12) {
          if (!i) return null;
          continue;
        }
        cleaned.push(area > 2 * Math.PI === (i === 0) ? [...ring].reverse() : ring);
      }
      return cleaned.length ? cleaned : null;
    }
    function normalized(geometry) {
      if (!geometry) return null;
      if (geometry.type === 'Polygon') {
        const rings = polygon(geometry.coordinates);
        return rings && { type: 'Polygon', coordinates: rings };
      }
      if (geometry.type === 'MultiPolygon') {
        return { type: 'MultiPolygon', coordinates: geometry.coordinates.map(polygon).filter(Boolean) };
      }
      return geometry;
    }
    async function layer(key) {
      const data = await load(key);
      const collection = data.type === 'Topology' ? topojson.feature(data, data.objects.features) : data;
      const features = collection.features || [{ type: 'Feature', geometry: collection, properties: {} }];
      const paths = [];
      for (const f of features) {
        const geom = normalized(f.geometry);
        if (!geom) continue;
        const labelOnly = key === 'provinces' || key === 'seas';
        const d = path(geom);
        if (!d) continue;
        if (!labelOnly) paths.push(d);
        if (!labelOnly && key !== 'lakes') continue;
        const area = path.area(geom),
          center = path.centroid(geom);
        const name = f.properties[languageKey] || f.properties.name;
        if (!name || area < (key === 'lakes' ? 320 : 3500) || !center.every(Number.isFinite)) continue;
        if (!d3.geoContains(geom, projection.invert(center))) continue;
        terrainLabels.push({ name, pos: center, area, kind: key });
      }
      return paths.join('');
    }
    let streetSvg = '';
    if (street) {
      try {
        streetSvg = await streetBasemap(projection, width, height,
          languageKey === 'name_zht' ? 'zh-Hant' : languageKey === 'name_zh' ? 'zh-Hans' : language, terrainLabels, deadline);
      } catch (error) {
        const confirmed = points.filter(point => point.osmId).map(point => point.name + ' ' + JSON.stringify(point.xy));
        throw Error(error.message + (confirmed.length ? ' Confirmed: ' + confirmed.join('; ') : ''));
      }
    }
    const [land, lakes, rivers, borders] = street ? [] : await Promise.all(
      ['land', 'lakes', 'rivers', 'borders', 'provinces', 'seas'].map(layer),
    );
    if (!street && !land) throw Error('No land intersects this regional map. Check the extent or use ocean geometry.');
    const labels = [];
    const markers = points
      .map(({ name, xy, osmId, osmName, osmTag }, index) => {
        const [x, y] = projection(xy);
        if (x < 0 || x > width || y < 0 || y > height) throw Error('The requested bounds exclude ' + name);
        const w = Math.min(width / 2, Array.from(name).length * 11 + 8);
        const offsets = [
          [11, -9],
          [11, 20],
          [-w - 11, -9],
          [-w - 11, 20],
          [11, -27],
          [-w - 11, 37],
        ];
        const candidates = offsets.map(([ox, oy]) => ({
          x: Math.max(4, Math.min(width - w - 4, x + ox)),
          y: Math.max(20, Math.min(height - 6, y + oy)),
          w,
        }));
        const label =
          candidates.find(
            (p) => !labels.some((q) => Math.abs(p.y - q.y) < 20 && p.x < q.x + q.w && q.x < p.x + w),
          ) || candidates[0];
        labels.push(label);
        return (
          '<g data-visual-marker="" data-map-stop="' + index + '" data-longitude="' + xy[0] +
          '" data-latitude="' + xy[1] + '"' + (osmId ? ' data-osm-id="' + esc(osmId) + '"' : '') + '><title>' +
          esc(name) + '</title>' + (osmId && osmName ? '<desc>' + esc(osmName) + ' · ' + esc(osmTag) + '</desc>' : '') + '<circle cx="' +
          x.toFixed(1) +
          '" cy="' +
          y.toFixed(1) +
          '" r="5" stroke-width="2"/><text x="' +
          label.x.toFixed(1) +
          '" y="' +
          label.y.toFixed(1) +
          '" stroke-width="3" stroke-linejoin="round" paint-order="stroke" font-size="16" font-weight="600">' +
          esc(name) +
          '</text></g>'
        );
      })
      .join('');
    const textWidth = (name) => Array.from(name).reduce((w, c) => w + (/[^\x00-\x7f]/.test(c) ? 13 : 7), 0);
    const context = [];
    for (const f of terrainLabels.sort((a, b) => b.area - a.area || a.name.localeCompare(b.name))) {
      const w = textWidth(f.name);
      const [cx, cy] = f.pos;
      const label = { x: cx - w / 2, y: cy, w };
      if (
        context.length >= 13 ||
        label.x < 8 ||
        label.x + w > width - 8 ||
        cy < 24 ||
        cy > height - 34 ||
        (label.x + w > width - 60 && cy < 65) ||
        labels.some((q) => Math.abs(cy - q.y) < 21 && label.x < q.x + q.w + 6 && q.x < label.x + w + 6)
      )
        continue;
      labels.push(label);
      context.push(
        '<text data-map-context="' +
          f.kind +
          '" x="' +
          cx.toFixed(1) +
          '" y="' +
          cy.toFixed(1) +
          '" text-anchor="middle" font-size="12">' +
          esc(f.name) +
          '</text>',
      );
    }
    const kmPerPixel =
      d3.geoDistance(
        projection.invert([width / 2, height / 2]),
        projection.invert([width / 2 + 1, height / 2]),
      ) * 6371;
    const power = 10 ** Math.floor(Math.log10(kmPerPixel * 90));
    const distance = ([5, 2, 1].find((x) => x * power <= kmPerPixel * 90) || 1) * power;
    const bar = distance / kmPerPixel;
    const scaleLabel = distance >= 1 ? distance + ' ' + strings.km : Math.round(distance * 1000) + ' ' + strings.m;
    const svg =
      '<svg xmlns="http://www.w3.org/2000/svg" data-visual-map="" data-visual-atlas-map="" role="img" aria-label="' +
      esc(title || points.map((p) => p.name).join(', ')) +
      '" viewBox="0 0 ' +
      width +
      ' ' +
      height +
      '">' +
      (street ? '<rect data-street-ground="" width="100%" height="100%"/>' + streetSvg : '<rect width="100%" height="100%"/>' +
      '<path data-visual-region="" data-map-layer="land" d="' +
      land +
      '" stroke-width=".9"/>' +
      '<path data-map-layer="borders" d="' +
      borders +
      '" fill="none" opacity=".46" stroke-dasharray="2.5 3"/>' +
      '<path data-map-layer="lakes" d="' +
      lakes +
      '"/>' +
      '<path data-map-layer="rivers" d="' +
      rivers +
      '" fill="none" stroke-width=".85" opacity=".45"/>') +
      context.join('') +
      markers +
      '<g data-map-north="" text-anchor="middle" font-size="12"><text x="' +
      (width - 33) +
      '" y="34">' +
      esc(strings.north) +
      '</text><path d="M' +
      (width - 33) +
      ' 51v-12m-4 4 4-4 4 4" fill="none" stroke="currentColor"/></g>' +
      '<g transform="translate(24 ' +
      (height - 18) +
      ')" data-map-scale="" font-size="12"><path d="M0 -4v4h' +
      bar.toFixed(1) +
      'v-4" fill="none" stroke="currentColor"/><text y="-8">' +
      esc(scaleLabel) +
      '</text></g></svg>';
    self.postMessage({ svg });
  } catch (e) {
    self.postMessage({ issue: String(e.message || e) });
  }
};
