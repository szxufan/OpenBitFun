// Verify the inputs used by the release build, without an installation or network.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';

const resource = name => new URL(name, import.meta.url);
async function manifest(name) {
  return JSON.parse(await readFile(resource(name), 'utf8'));
}
async function verify(path, hash) {
  const bytes = await readFile(resource('../third_party/visualize/' + path));
  assert.equal(createHash('sha256').update(bytes).digest('hex'), hash, path);
}
for (const [name, layer] of Object.entries(await manifest('./visualize-regional-atlas.json'))) {
  await verify('atlas/' + name + '.json', layer.packagedSha256);
}
for (const [name, asset] of Object.entries(await manifest('./visualize-runtime.json'))) {
  if (new URL(asset.url).hostname !== 'persistent.oaistatic.com') await verify('runtime/' + name, asset.sha256);
}
const skill = await readFile(resource('../src/crates/assembly/core/builtin_skills/visualize/SKILL.md'), 'utf8');
assert.match(skill, /^name: visualize$/m);
assert.match(skill, /data-visual-atlas="1"/);
console.log('Verified the bundled FlowChat skill, guest dependencies and offline atlas.');
