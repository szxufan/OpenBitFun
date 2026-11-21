// Real NSOpenPanel proxy identity, semantic input, sibling isolation and Stop.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

assert.equal(process.platform, 'darwin', 'file panel fixture requires macOS');
const root = fileURLToPath(new URL('../', import.meta.url));
const directory = await mkdtemp(join(process.env.OPENBITFUN_FIXTURE_DIR || tmpdir(), 'openbitfun-file-panel-'));
const run = (command, args, env = process.env) => new Promise((resolve, reject) => {
  const child = spawn(command, args, { cwd: root, stdio: 'inherit', windowsHide: true, env, timeout: 120000, killSignal: 'SIGKILL' });
  child.once('error', reject);
  child.once('close', code => code === 0 ? resolve() : reject(new Error(`${command} exited ${code}`)));
});
let fixture;
try {
  const binary = join(directory, 'file-panel-fixture');
  const state = join(directory, 'state.json');
  const media = join(directory, 'media');
  await mkdir(join(media, 'fixture-child'), { recursive: true });
  await run('xcrun', ['clang', '-fobjc-arc', '-Wall', '-Werror', 'scripts/fixtures/computer-use-file-panel.m', '-framework', 'AppKit', '-o', binary]);
  const test = ['native_file_panel_target_fixture', '--ignored', '--nocapture', '--test-threads=1'];
  if (!process.env.OPENBITFUN_TEST_BINARY) {
    await run('cargo', ['test', '--locked', '-p', 'openbitfun-computer-use', '--lib', test[0], '--no-run']);
  }
  fixture = spawn(binary, [state, media], { cwd: root, stdio: ['ignore', 'pipe', 'inherit'], windowsHide: true });
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('File panel fixture did not become ready')), 15000);
    let output = '';
    fixture.once('error', error => { clearTimeout(timer); reject(error); });
    fixture.once('exit', code => { clearTimeout(timer); reject(new Error(`Fixture exited ${code}`)); });
    fixture.stdout.on('data', chunk => {
      output += String(chunk);
      if (/READY \d+/.test(output)) { clearTimeout(timer); resolve(); }
    });
  });
  await run(process.env.OPENBITFUN_TEST_BINARY || 'cargo',
    process.env.OPENBITFUN_TEST_BINARY ? test : ['test', '--locked', '-p', 'openbitfun-computer-use', '--lib', test[0], '--', ...test.slice(1)],
    { ...process.env, OPENBITFUN_FILE_PANEL_STATE: state });
} finally {
  if (fixture && fixture.exitCode === null) {
    await new Promise(resolve => { fixture.once('exit', resolve); fixture.kill('SIGTERM'); });
  }
  await rm(directory, { recursive: true, force: true });
}
