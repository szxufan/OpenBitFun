// Cache versioned public runtime dependencies for a local Visualize installation.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdir, mkdtemp, readFile, rename, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = fileURLToPath(new URL('../.visualize-runtime/', import.meta.url));
const release = JSON.parse(await readFile(new URL('./visualize-runtime.json', import.meta.url)));
const run = promisify(execFile);
const check = process.argv.includes('--check');
await mkdir(root, { recursive: true });
async function verify(file, hash) {
  assert.equal(
    createHash('sha256')
      .update(await readFile(file))
      .digest('hex'),
    hash,
    'Unexpected content: ' + path.basename(file),
  );
}
for (const [name, { url, sha256 }] of Object.entries(release)) {
  const file = path.join(root, name);
  try {
    await verify(file, sha256);
    continue;
  } catch (error) {
    if (check || error.code !== 'ENOENT') throw error;
  }
  const stage = await mkdtemp(path.join(root, 'download-'));
  try {
    const staged = path.join(stage, name);
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
        '60',
        '--fail',
        '--silent',
        '--show-error',
        '--location',
        url,
        '--output',
        staged,
      ],
      { windowsHide: true },
    );
    await verify(staged, sha256);
    await rename(staged, file);
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
}
console.log('Verified local visualization runtime dependencies: ' + Object.keys(release).length);
