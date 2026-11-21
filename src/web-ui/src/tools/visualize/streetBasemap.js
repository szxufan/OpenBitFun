// Snapshot only the visible public OSM tiles during publication. The opaque
// figure receives SVG geometry, never a URL or a bridge for requesting tiles.
import { VectorTile } from '@mapbox/vector-tile';
import { PbfReader } from 'pbf';

const tileOrigin = 'https://tiles.versatiles.org/tiles/osm-vector';
const unavailable = 'Street basemap unavailable. Explain that street detail is unavailable and show the stops as a timeline without a map or scale.';

async function tileBytes(url, deadline) {
  // One interrupted tile used to discard every verified stop in the viewport.
  // Retry that tile alone, within the projection worker's publication deadline.
  for (let attempt = 0; attempt < 2; attempt++) {
    const remaining = deadline - Date.now();
    if (remaining <= 0) break;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), remaining);
    try {
      const response = await fetch(url, { signal: controller.signal, credentials: 'omit', referrerPolicy: 'no-referrer', redirect: 'error' });
      if (response.ok) return new Uint8Array(await response.arrayBuffer());
      if (response.headers.has('retry-after') || (response.status !== 408 && response.status < 500)) break;
    } catch {
      // A dropped connection may also interrupt reading an otherwise valid response.
    } finally {
      clearTimeout(timer);
    }
  }
  throw Error(unavailable);
}

function tilePaths(tile, size, left, top, width, height, language, terrainLabels) {
  const paths = new Map();
  for (const layerName of ['land', 'ocean', 'water_polygons', 'water_lines', 'streets', 'water_polygons_labels', 'place_labels', 'street_labels_points']) {
    const layer = tile.layers[layerName];
    if (!layer) continue;
    for (let i = 0; i < layer.length; i++) {
      const feature = layer.feature(i);
      const { kind } = feature.properties;
      const factor = size / feature.extent;
      if (feature.type === 1) {
        const name = feature.properties['name_' + language] || feature.properties.name;
        if (!name) continue;
        const pt = feature.loadGeometry()[0][0];
        terrainLabels.push({ name, pos: [left + pt.x * factor, top + pt.y * factor], kind: layerName,
          area: layerName === 'water_polygons_labels' ? 3 : layerName === 'place_labels' ? 2 : 1 });
        continue;
      }
      if (layerName === 'land' && !['wood', 'forest', 'park', 'grass', 'grassland'].includes(kind)) continue;
      if (layerName === 'streets' && ['service', 'parking_aisle', 'driveway'].includes(kind)) continue;
      // A snapshot stores the viewport, not complete tiles extending beyond
      // its edges. Keep whole intersecting features so polygon holes survive.
      const [xmin, ymin, xmax, ymax] = feature.bbox();
      if (left + xmax * factor < 0 || left + xmin * factor > width ||
          top + ymax * factor < 0 || top + ymin * factor > height) continue;
      let paint = layerName;
      if (layerName === 'streets' && ['motorway', 'trunk', 'primary', 'secondary'].includes(kind)) paint = 'main-roads';
      const parts = paths.get(paint) ?? [];
      for (const line of feature.loadGeometry()) {
        const points = [];
        for (const pt of line) {
          const xy = Math.round(pt.x * factor) + ',' + Math.round(pt.y * factor);
          if (xy !== points[points.length - 1]) points.push(xy);
        }
        if (points.length >= (feature.type === 3 ? 3 : 2)) {
          parts.push('M' + points.join(' ') + (feature.type === 3 ? 'Z' : ''));
        }
      }
      paths.set(paint, parts);
    }
  }
  return [...paths].filter(([, parts]) => parts.length).map(([layer, parts]) =>
    '<path data-street-layer="' + layer + '" d="' + parts.join('') + '"/>').join('');
}

export async function streetBasemap(projection, width, height, language, terrainLabels, deadline = Date.now() + 15000) {
  const world = projection.scale() * 2 * Math.PI;
  // The public Shortbread tiles end at z14. Project their vector geometry at
  // the requested scale for close-up walking routes.
  const zoom = Math.min(14, Math.floor(Math.log2(world / 256)));
  if (zoom < 9) throw Error('Street maps need a city-scale extent. Use the regional atlas for distant places.');
  const count = 2 ** zoom;
  const tileSize = world / count;
  const [lon, lat] = projection.invert([0, 0]);
  const x0 = (lon + 180) / 360 * count;
  const y0 = (1 - Math.asinh(Math.tan(lat * Math.PI / 180)) / Math.PI) / 2 * count;
  const tiles = [];
  for (let y = Math.max(0, Math.floor(y0)); y < Math.min(count, Math.ceil(y0 + height / tileSize)); y++) {
    for (let x = Math.floor(x0); x < Math.ceil(x0 + width / tileSize); x++) tiles.push({ x, y });
  }
  const output = [];
  // Bounded concurrency and deterministic paint/label order, even on slow links.
  for (let i = 0; i < tiles.length; i += 4) {
    const batch = await Promise.all(tiles.slice(i, i + 4).map(async ({ x, y }) => {
      const url = tileOrigin + '/' + zoom + '/' + ((x + count) % count) + '/' + y;
      const tile = new VectorTile(new PbfReader(await tileBytes(url, deadline)));
      const left = (x - x0) * tileSize, top = (y - y0) * tileSize;
      const labels = [];
      const paths = tilePaths(tile, tileSize, left, top, width, height, language, labels);
      return { svg: '<svg x="' + left.toFixed(2) + '" y="' + top.toFixed(2) + '" width="' +
        tileSize.toFixed(2) + '" height="' + tileSize.toFixed(2) + '" overflow="hidden">' + paths + '</svg>', labels };
    }));
    for (const { svg, labels } of batch) {
      output.push(svg);
      terrainLabels.push(...labels);
    }
  }
  const svg = output.join('');
  if (!svg.includes('data-street-layer=')) throw Error(unavailable);
  return svg;
}
