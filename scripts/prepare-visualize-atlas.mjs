// Derive offline drawing layers from pinned public geometry and a multilingual gazetteer.
import { readFile, writeFile, mkdir, rename } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import assert from 'node:assert/strict';
const run = promisify(execFile);
const local = new URL('../.visualize-runtime/regional/', import.meta.url);
const manifest = JSON.parse(await readFile(new URL('./visualize-regional-atlas.json', import.meta.url)));
const output = new URL('../third_party/visualize/atlas/', import.meta.url);
await mkdir(output, { recursive: true });
await mkdir(new URL('sources/', local), { recursive: true });
const hash = (data) => createHash('sha256').update(data).digest('hex');
async function install(name, data) {
  const dest = new URL(name, output);
  const installed = await readFile(dest).catch((error) => {
    if (error.code !== 'ENOENT') throw error;
  });
  if (installed && hash(installed) === hash(data)) return;
  const tmp = new URL(name + '.' + randomUUID() + '.tmp', output);
  await writeFile(tmp, data);
  await rename(tmp, dest);
}
for (const [key, entry] of Object.entries(manifest)) {
  const src = new URL('sources/' + key + '.json', local);
  let data;
  try {
    data = await readFile(src);
  } catch (e) {
    if (e.code !== 'ENOENT') throw e;
    const tmp = fileURLToPath(src) + '.' + randomUUID() + '.tmp';
    await run(
      'curl',
      [
        '--proto',
        '=https',
        '--proto-redir',
        '=https',
        '--retry',
        '2',
        '--max-time',
        '120',
        '-fsSL',
        entry.url,
        '-o',
        tmp,
      ],
      { windowsHide: true },
    );
    data = await readFile(tmp);
    assert.equal(hash(data), entry.sha256, key);
    await rename(tmp, src);
  }
  assert.equal(hash(data), entry.sha256, key);
  if (key === 'places') {
    const rows = JSON.parse(data).features.map(({ properties: p, geometry: g }) => [
      p.ISO_A2,
      p.ADM1NAME,
      [...new Set([p.NAME, p.NAMEASCII, p.NAME_EN, p.NAME_ZH, p.NAME_ZHT].filter(Boolean))],
      g.coordinates,
    ]);
    data = JSON.stringify(rows);
  } else if (key === 'provinces' || key === 'seas') {
    const features = JSON.parse(data).features.map(({ properties: p, geometry }) => ({
      type: 'Feature',
      geometry,
      properties: { name: p.name_en || p.name, name_zh: p.name_zh, name_zht: p.name_zht },
    }));
    data = JSON.stringify({ type: 'FeatureCollection', features });
  } else if (key === 'borders') {
    data = JSON.stringify({
      type: 'GeometryCollection',
      geometries: JSON.parse(data).features.map((f) => f.geometry),
    });
  }
  assert.equal(hash(data), entry.packagedSha256, key + ' packaged');
  await install(key + '.json', data);
}
console.log('Refreshed packaged regional map layers.');
