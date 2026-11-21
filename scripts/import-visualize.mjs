import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { cp, mkdir, mkdtemp, readFile, readdir, rename, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const manifest = JSON.parse(await readFile(new URL('./visualize-upstream.json', import.meta.url), 'utf8'));
const owner = path.join(root, 'src/crates/assembly/core/builtin_skills/visualize');
const target = path.join(owner, 'upstream');
const cache = path.join(root, '.visualize-runtime');
const args = process.argv.slice(2);
const check = args.includes('--check');
const paths = args.filter((arg) => arg !== '--check');
if (paths.length > 1) throw Error('Usage: node scripts/import-visualize.mjs [--check] [plugin-directory]');
const source =
  paths[0] ??
  path.join(
    process.env.CODEX_HOME ?? path.join(os.homedir(), '.codex'),
    'plugins/cache/openai-curated-remote/visualize',
    manifest.version,
  );

async function verify(directory) {
  const found = [];
  async function walk(relative) {
    for (const entry of await readdir(path.join(directory, relative), { withFileTypes: true })) {
      const name = path.posix.join(relative, entry.name);
      if (entry.isDirectory()) await walk(name);
      else {
        assert.ok(entry.isFile(), 'Unexpected non-regular file: ' + name);
        const bytes = await readFile(path.join(directory, name));
        assert.equal(
          createHash('sha256').update(bytes).digest('hex'),
          manifest.files[name],
          'Unexpected file or content: ' + name,
        );
        found.push(name);
      }
    }
  }
  await walk('');
  assert.deepEqual(found.sort(), Object.keys(manifest.files).sort(), 'Missing upstream files');
}

const installed = await readFile(path.join(target, '.codex-plugin/plugin.json'), 'utf8')
  .then(JSON.parse)
  .catch(error => {
    if (error.code !== 'ENOENT') throw error;
  });
if (!check && (paths.length || installed?.version !== manifest.version)) await verify(source);
if (!check && installed?.version !== manifest.version) {
  // Keep the previous local install outside the embedded skill tree during upgrades.
  await mkdir(cache, { recursive: true });
  const stage = await mkdtemp(path.join(cache, 'import-'));
  const backup = path.join(stage, 'previous');
  let preserveBackup = false;
  try {
    await cp(source, path.join(stage, 'package'), { recursive: true, preserveTimestamps: true });
    await verify(path.join(stage, 'package'));
    if (installed) await rename(target, backup);
    try {
      await rename(path.join(stage, 'package'), target);
    } catch (error) {
      if (installed) {
        preserveBackup = true;
        await rename(backup, target);
        preserveBackup = false;
      }
      throw error;
    }
  } finally {
    if (!preserveBackup) await rm(stage, { recursive: true, force: true });
  }
}
await verify(target);

// Before this importer cleaned its staging directory, a successful upgrade
// left exactly one "previous" install. Leave interrupted swaps and unrelated
// cache entries alone. Only prune after verifying the current install.
if (!check) {
  const stages = await readdir(cache, { withFileTypes: true }).catch(error => {
    if (error.code === 'ENOENT') return [];
    throw error;
  });
  for (const stage of stages.filter(entry => entry.isDirectory() && /^import-[A-Za-z0-9]{6}$/.test(entry.name))) {
    const dir = path.join(cache, stage.name);
    const entries = await readdir(dir, { withFileTypes: true }).catch(() => []);
    if (entries.length !== 1 || entries[0].name !== 'previous' || !entries[0].isDirectory()) continue;
    const plugin = await readFile(path.join(dir, 'previous/.codex-plugin/plugin.json'), 'utf8').then(JSON.parse).catch(() => null);
    if (plugin?.name === 'visualize' && typeof plugin.version === 'string' && plugin.version !== manifest.version)
      await rm(dir, { recursive: true, force: true });
  }
}

// Keep the packaged guides and validation workflow intact. Only the resource
// location and the final reference differ between Codex and FlowChat.
const guidePrefix = 'skills/visualize/';
const uri = 'openbitfun://current-session/artifacts/visualizations/v2/<title>.html';
const streetMaps = 'For a geographic map write to map-<slug>.html. For a map of sightseeing, walking or transit stops within one city, use detail:"street", city, optional ISO country and the ordered public stop names in places. Submit the names directly. The host resolves stops, fetches the visible tiles, and saves the verified map for offline replay. Use a short local query and a matching osmTag (railway:station or highway:pedestrian for those stop types), or a reported osmId to identify the intended place. Ask about unresolved ambiguity. Sourced WGS84 coordinates are also accepted. Do not invent roads, shorelines, route geometry or scales. For a missing-map follow-up write the atlas declaration again in the current turn using the earlier stops. Repair it with the verified candidate coordinates in the current receipt. If name lookup fails for a public stop, get sourced WGS84 coordinates with WebSearch or WebFetch and resubmit the same map with a source credit. If coordinates or tiles remain unavailable, report the current error in text. An itinerary request may still show its schedule as an <ol data-visual-itinerary>. For explicitly offline or private locations use sourced local geometry. For figures with bound data supply stop coordinates or local geometry.';

function flowChatEntry(upstream, resourceDirectory) {
  const adaptedMap = resourceDirectory.startsWith('..') ? './maps.md' : './authoring/maps.md';
  return upstream
    .replace(
      '`<skill-dir>` is this SKILL.md’s directory; resolve relative file paths from it.',
      '`<skill-dir>` is `' + resourceDirectory + '` relative to this SKILL.md. Resolve paths starting with `<skill-dir>/` from there. Resolve Markdown links from the directory of this SKILL.md.',
    )
    .replace(/\]\(mockups\.md\)/g, '](' + resourceDirectory + '/mockups.md)')
    .replace('For geographic maps, including maps within another visualization, read [maps.md](maps.md) for sourced geometry, projection, and map-specific validation.',
      'For ordinary city marker maps, the inline atlas declaration in “Publish in FlowChat” is the complete workflow. Submit it directly. For additional geographic layers or projections, read the FlowChat [maps guide](' + adaptedMap + ').')
    .replace(/### File[\s\S]*?(?=### Fragment)/,
      '### Publish in FlowChat\n\n' +
      '- An explicitly requested webpage (网页), website, or project page is a normal project file. Deliver a working independently openable page and link its entry file.\n' +
      '- For an inline HTML figure, write the literal fragment directly with Write. The first line is `+++ ' + uri + '` (replace <title> with an ASCII lowercase slug). The next line starts the markup. This is your only HTML destination. Use Edit on that URI for corrections.\n' +
      '- FlowChat opens this exact source privately with the conversation renderer during publication. Write/Edit returns the render result. Repair reported issues on the same URI, then use the returned reference. All preview instructions in the guides, including “open it before replying” for maps, are fulfilled by this host check. Do not create scratch HTML, launch a server, open a browser, request screenshots, or produce tool-by-tool progress messages for verification.\n' +
      '- Choose a day calendar or timeline for a time-based itinerary. For today or tonight, verify the destination time and offer start times still available. An adjustable arrival time can shift its events and return time together. Choose a map for spatial questions. When both matter, combine them. For a city or regional marker map, publish the bundled offline atlas inline first. Write a heading and `<script type="application/json" data-visual-atlas="1">{"places":[{"name":"京都","country":"JP"},{"name":"大阪","country":"JP"}]}</script>` using the requested city names. Support for worldwide land, coasts, lakes, rivers, province boundaries, city coordinates, labels and projection is built in. It produces a complete standalone SVG in the saved reply. No downloads, npm, scratch scripts or local previews are necessary. ' + streetMaps + ' Follow the adapted maps guide for other map types. For a moving simulation put data-visual-motion on the moving element or canvas.\n\n')
    .replace(/- Check that JavaScript has no undefined identifiers[^\n]+/,
      '- Check selectors, JavaScript identifiers and the primary interaction. Use the FlowChat publication result for the render check.')
    .replace(
      'using the absolute executor-side file path:',
      'using the session URI from the successful Write or Edit:',
    )
    .replaceAll('<absolute-path>/<title>.html', uri);
}

async function syncGuide(destination, expected) {
  const installed = await readFile(destination, 'utf8').catch((error) => {
    if (error.code !== 'ENOENT') throw error;
  });
  if (check) assert.equal(installed === expected, true, 'Missing or stale guide: ' + path.relative(owner, destination));
  else if (installed !== expected) {
    await mkdir(path.dirname(destination), { recursive: true });
    const stage = await mkdtemp(path.join(root, '.visualize-import-'));
    try {
      await writeFile(path.join(stage, 'guide'), expected);
      await rename(path.join(stage, 'guide'), destination);
    } finally {
      await rm(stage, { recursive: true, force: true });
    }
  }
}

const packagedGuides = path.join(target, guidePrefix);
for (const entry of Object.keys(manifest.files)) {
  if (!entry.startsWith(guidePrefix) || !entry.endsWith('.md')) continue;
  const name = entry.slice(guidePrefix.length);
  const expected = name === 'SKILL.md'
    ? flowChatEntry(await readFile(path.join(packagedGuides, name), 'utf8'), '../upstream/skills/visualize')
    : name === 'maps.md'
      ? (await readFile(path.join(target, entry), 'utf8'))
        .replace('# Maps', '# Maps\n\nFor city, regional and multi-city marker maps use the packaged regional atlas. Write the diagram once to the FlowChat URI with a heading and `<script type="application/json" data-visual-atlas="1">{"places":[{"name":"京都","country":"JP"},{"name":"大阪","country":"JP"}]}</script>`. A place takes its local name and optional ISO-2 country, first-level region, or sourced coordinates [longitude, latitude]. Optional bounds are [west,south,east,north]. Use this same path for any covered region. It already supplies real coastlines, rivers, lakes and boundaries. ' + streetMaps + ' For a custom projected map mark the SVG with data-visual-map and data-map-source (source URL), and its real basemap geometry with data-visual-region.')
        .replace('Open it before replying and fix', 'The background Write/Edit check opens the final fragment before the reply. Use its diagnostics to fix')
      : await readFile(path.join(target, entry), 'utf8');
  await syncGuide(path.join(owner, 'authoring', name), expected);
}

console.log(
  'Verified Codex Visualize ' +
    manifest.version +
    ' (' +
    Object.keys(manifest.files).length +
    ' files, SHA-256).',
);
