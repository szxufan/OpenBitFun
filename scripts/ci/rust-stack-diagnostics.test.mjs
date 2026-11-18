import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { debuggerPlan, loaderEnvironment, overflowThread, runCargoTests, runProcess } from './rust-stack-diagnostics.mjs';

const testName = 'some_module::tests::large_future';
const ok = { code: 0, signal: null, timedOut: false };
function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'openbitfun-stack-diagnostic-'));
  t.after(() => {
    assert.equal(path.dirname(root), path.resolve(tmpdir()));
    rmSync(root, { recursive: true, force: true });
  });
  const artifact = {
    reason: 'compiler-artifact', profile: { test: true, debuginfo: 0 },
    features: ['custom-feature'], executable: path.join(root, 'target', 'debug', 'deps', 'tests'),
    manifest_path: path.join(root, 'crate', 'Cargo.toml'),
  };
  return { root, artifact, calls: [], env: { GITHUB_OUTPUT: path.join(root, 'outputs') } };
}

function fakeRunner(f, { code = 101, thread = testName, match = true,
  extraArtifact, gdbMissing = false, listFails = false, diagnosticThrows = false } = {}) {
  return async (command, args, options) => {
    f.calls.push({ command, args, ...options });
    const emit = (line, stream = 'stdout') => options.onLine?.(line, stream);
    if (command === 'cargo') {
      emit(JSON.stringify(f.artifact));
      if (extraArtifact) emit(JSON.stringify(extraArtifact));
      emit('{"message":"test JSON must not disappear"}');
      emit('compiler warning', 'stderr');
      if (thread) emit(`thread '${thread}' (7057) has overflowed its stack`, 'stderr');
      return { ...ok, code };
    }
    if (diagnosticThrows) throw new Error('diagnostic fixture failure');
    if (command === 'rustc' && args.includes('target-libdir')) emit('/toolchain/lib');
    if (args.includes('--list')) {
      emit(`${match ? testName : `${testName}_other`}: test`);
      return { ...ok, code: listFails ? 1 : 0 };
    }
    if (command === 'gdb' && gdbMissing) return { ...ok, code: 1, error: 'spawn gdb ENOENT' };
    emit('running 1 test');
    return ok;
  };
}

async function execute(f, overrides = {}) {
  return runCargoTests({ cargoArgs: ['--locked', '-p', 'example', '--features', 'custom-feature', '--lib'],
    outputDir: f.root, env: f.env, platform: 'linux', print: () => {},
    run: fakeRunner(f), ...overrides });
}

test('recognizes old and current Rust overflow signatures but not ordinary panic text', () => {
  assert.equal(overflowThread(`thread '${testName}' has overflowed its stack`), testName);
  assert.equal(overflowThread(`thread '${testName}' (7057) has overflowed its stack`), testName);
  assert.equal(overflowThread(`thread '${testName}' panicked at source.rs:1`), undefined);
});

for (const platform of ['linux', 'darwin', 'win32']) {
  for (const [label, code, thread] of [['success', 0, testName], ['ordinary failure', 101, null]]) {
    test(`${platform} ${label} runs Cargo exactly once with no diagnostics or stack override`, async (t) => {
      const f = fixture(t);
      assert.equal(await execute(f, { platform, run: fakeRunner(f, { code, thread }) }), code);
      assert.equal(f.calls.length, 1);
      assert.equal(f.calls[0].command, 'cargo');
      assert.deepEqual(f.calls[0].args, ['test', '--message-format=json-render-diagnostics',
        '--locked', '-p', 'example', '--features', 'custom-feature', '--lib']);
      assert.equal(f.calls[0].env.RUST_MIN_STACK, undefined);
      assert.equal(existsSync(f.env.GITHUB_OUTPUT), false);
      assert.equal(existsSync(path.join(f.root, 'summary.json')), false);
      assert.match(readFileSync(path.join(f.root, 'cargo-test.log'), 'utf8'), /test JSON must not disappear/);
    });
  }
}

test('loader paths retain platform defaults and avoid duplicate Windows PATH keys', () => {
  assert.equal(loaderEnvironment('linux', { LD_LIBRARY_PATH: '/old' }, ['/new']).LD_LIBRARY_PATH, '/new:/old');
  assert.equal(loaderEnvironment('darwin', { HOME: '/Users/test' }, ['/new']).DYLD_FALLBACK_LIBRARY_PATH,
    '/new:/Users/test/lib:/usr/local/lib:/usr/lib');
  const env = { Path: 'C:\\old', PATH: 'C:\\duplicate' };
  const windows = loaderEnvironment('win32', env, ['C:\\new']);
  assert.equal(windows.Path, 'C:\\new;C:\\old');
  assert.equal(Object.hasOwn(windows, 'PATH'), false);
  assert.equal(env.PATH, 'C:\\duplicate');
});

test('macOS debugger uses batch crash commands and supplies the inferior loader path', () => {
  const plan = debuggerPlan('darwin', '/test path/tests', [testName, '--exact'],
    { DYLD_FALLBACK_LIBRARY_PATH: '/library path:/usr/lib' });
  assert.equal(plan.command, 'lldb');
  assert.ok(plan.args.includes('--no-lldbinit'));
  assert.ok(plan.args.includes('--batch'));
  assert.ok(plan.args.includes('settings set target.env-vars "DYLD_FALLBACK_LIBRARY_PATH=/library path:/usr/lib"'));
  assert.deepEqual(plan.args.slice(plan.args.indexOf('-k'), plan.args.indexOf('--')),
    ['-k', 'thread backtrace -c 80 all', '-k', 'register read', '-k', 'disassemble --pc --count 16']);
  assert.deepEqual(plan.args.slice(-4), ['--', '/test path/tests', testName, '--exact']);
});

test('Windows discovers the SDK console debugger and uses local symbols and an explicit command file', () => {
  const expected = 'C:\\Program Files (x86)\\Windows Kits\\10\\Debuggers\\x64\\cdb.exe';
  const plan = debuggerPlan('win32', 'C:\\test path\\tests.exe', [testName],
    { 'ProgramFiles(x86)': 'C:\\Program Files (x86)', _NT_SYMBOL_PATH: 'srv*https://example.test/symbols' },
    'C:\\logs\\cdb-commands.txt', { arch: 'x64', fileExists: (file) => file === expected });
  assert.equal(plan.command, expected);
  assert.deepEqual(plan.args, ['-G', '-y', 'C:\\test path', '-cf', 'C:\\logs\\cdb-commands.txt', 'C:\\test path\\tests.exe', testName]);
  assert.equal(plan.env._NT_SYMBOL_PATH, '');
  assert.equal(plan.env._NT_ALT_SYMBOL_PATH, '');
  for (const exception of ['sov', 'av', '0xc0000409', '0x40000015']) assert.ok(plan.commands.includes(`" ${exception}`));
  assert.ok(plan.commands.includes('~* kb 0n80'));
  assert.ok(plan.commands.endsWith('\ng'));
});

for (const [platform, debuggerName] of [['darwin', 'lldb'], ['win32', 'cdb']]) {
  test(`${platform} reruns preserve failure and continue after a missing debugger`, async (t) => {
    const f = fixture(t);
    const fake = fakeRunner(f);
    const run = async (command, args, options) => {
      const result = await fake(command, args, options);
      if (command === 'lldb' || command.endsWith('cdb.exe')) return { ...ok, code: 1, error: 'ENOENT' };
      return result;
    };
    assert.equal(await execute(f, { platform, run }), 101);
    assert.equal(f.calls.filter((call) => call.command === 'cargo').length, 1);
    const report = JSON.parse(readFileSync(path.join(f.root, 'summary.json'), 'utf8'));
    assert.equal(report.debugger, debuggerName);
    assert.equal(report.stages.find((stage) => stage.name === debuggerName).error, 'ENOENT');
    assert.equal(report.stages.at(-1).name, 'large-stack');
    assert.equal(f.calls.at(-1).env.RUST_MIN_STACK, '16777216');
    if (platform === 'win32') assert.match(readFileSync(path.join(f.root, 'cdb-commands.txt'), 'utf8'), /sxe/);
  });
}

test('overflow uses the exact built artifact and package cwd, retaining original failure after passing reruns', async (t) => {
  const f = fixture(t);
  f.env.RUST_MIN_STACK = '2097152';
  assert.equal(await execute(f), 101);
  assert.equal(f.calls.filter((call) => call.command === 'cargo').length, 1);
  const reruns = f.calls.filter((call) => call.command === f.artifact.executable && !call.args.includes('--list'));
  assert.equal(reruns.length, 2);
  for (const call of reruns) {
    assert.deepEqual(call.args, [testName, '--exact', '--include-ignored', '--nocapture', '--test-threads=1']);
    assert.equal(call.cwd, path.dirname(f.artifact.manifest_path));
    assert.ok(call.timeoutMs > 0 && call.timeoutMs <= 30_000);
  }
  assert.equal(reruns[0].env.RUST_MIN_STACK, '2097152');
  assert.equal(reruns[1].env.RUST_MIN_STACK, '16777216');
  assert.equal(f.env.RUST_MIN_STACK, '2097152');
  const gdb = f.calls.find((call) => call.command === 'gdb');
  assert.ok(gdb.args.includes('thread apply all bt 80'));
  assert.ok(gdb.args.includes('handle SIGSEGV stop print nopass'));
  assert.equal(gdb.env.RUST_MIN_STACK, '2097152');
  assert.ok(gdb.timeoutMs <= 60_000);
  assert.match(readFileSync(f.env.GITHUB_OUTPUT, 'utf8'), /stack_overflow=true/);
  const report = JSON.parse(readFileSync(path.join(f.root, 'summary.json'), 'utf8'));
  assert.equal(report.original.code, 101);
  assert.equal(report.selectedExecutable, f.artifact.executable);
  assert.deepEqual(report.artifacts[0].features, ['custom-feature']);
});

test('Windows stack-overflow NTSTATUS is preserved without assuming Cargo exits with 101', async (t) => {
  const f = fixture(t);
  const status = 0xc00000fd;
  assert.equal(await execute(f, { platform: 'win32', run: fakeRunner(f, { code: status }) }), status);
  const report = JSON.parse(readFileSync(path.join(f.root, 'summary.json'), 'utf8'));
  assert.equal(report.original.code, status);
});

for (const status of [0xc00000fd, -1073741571]) {
  test(`Windows unnamed stack overflow (${status}) retains evidence without guessing a test`, async (t) => {
    const f = fixture(t);
    assert.equal(await execute(f, {
      platform: 'win32', run: fakeRunner(f, { code: status, thread: null }),
    }), status);
    const report = JSON.parse(readFileSync(path.join(f.root, 'summary.json'), 'utf8'));
    assert.equal(report.thread, null);
    assert.match(report.note, /without a named test thread/);
    assert.equal(f.calls.some((call) => call.args.includes('--list')), false);
    assert.match(readFileSync(f.env.GITHUB_OUTPUT, 'utf8'), /stack_overflow=true/);
  });
}

for (const [label, options] of [
  ['worker thread', { thread: 'tokio-runtime-worker', match: false }],
  ['substring only', { match: false }],
  ['listing failure', { listFails: true }],
]) {
  test(`${label} preserves evidence without running zero tests as a reproduction`, async (t) => {
    const f = fixture(t);
    assert.equal(await execute(f, { run: fakeRunner(f, options) }), 101);
    assert.equal(f.calls.some((call) => call.command === 'gdb'), false);
    assert.equal(f.calls.some((call) => call.args.includes('--nocapture')), false);
    assert.ok(existsSync(path.join(f.root, 'summary.json')));
  });
}

test('ambiguous test names across targets do not select an arbitrary binary', async (t) => {
  const f = fixture(t);
  const extraArtifact = { ...f.artifact, executable: `${f.artifact.executable}-other` };
  assert.equal(await execute(f, { run: fakeRunner(f, { extraArtifact }) }), 101);
  assert.equal(f.calls.filter((call) => call.args.includes('--list')).length, 2);
  assert.equal(f.calls.some((call) => call.command === 'gdb'), false);
});

test('missing GDB still permits the large-stack comparison and records its error', async (t) => {
  const f = fixture(t);
  assert.equal(await execute(f, { run: fakeRunner(f, { gdbMissing: true }) }), 101);
  const report = JSON.parse(readFileSync(path.join(f.root, 'summary.json'), 'utf8'));
  assert.match(report.stages.find((stage) => stage.name === 'gdb').error, /ENOENT/);
  assert.equal(report.stages.at(-1).name, 'large-stack');
});

test('diagnostic exceptions preserve the original failure and partial report', async (t) => {
  const f = fixture(t);
  assert.equal(await execute(f, { run: fakeRunner(f, { diagnosticThrows: true }) }), 101);
  const report = JSON.parse(readFileSync(path.join(f.root, 'summary.json'), 'utf8'));
  assert.match(report.diagnosticError, /diagnostic fixture failure/);
});

test('subprocess output streams both channels and preserves a nonzero exit', async () => {
  const lines = [];
  const result = await runProcess(process.execPath, ['-e',
    'process.stdout.write("out\\n"); process.stderr.write("err\\n"); process.exitCode = 7;'],
  { onLine: (line, stream) => lines.push([line, stream]) });
  assert.equal(result.code, 7);
  assert.deepEqual(lines.sort(), [['err', 'stderr'], ['out', 'stdout']]);
});

test('subprocess timeout terminates a stalled diagnostic', async () => {
  const result = await runProcess(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { timeoutMs: 100 });
  assert.equal(result.timedOut, true);
  assert.notEqual(result.code, 0);
});

test('native timeout terminates diagnostic descendants as well as the parent', { timeout: 10_000 }, async () => {
  let descendant;
  try {
    const result = await runProcess(process.execPath, ['-e', `
      const { spawn } = require('node:child_process');
      const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {
        windowsHide: true, stdio: 'ignore'
      });
      console.log(child.pid);
      setInterval(() => {}, 1000);
    `], { timeoutMs: 1_000, onLine: (line) => { if (/^\d+$/.test(line)) descendant = Number(line); } });
    assert.ok(descendant, 'The fixture must start its descendant before the timeout');
    assert.equal(result.timedOut, true);
    assert.equal(result.cleanupError, undefined);
    let alive = true;
    for (let attempt = 0; attempt < 50 && alive; attempt++) {
      try {
        process.kill(descendant, 0);
        // Linux may retain a dead, reparented zombie briefly until it is reaped.
        if (process.platform === 'linux') {
          alive = !/\) Z /.test(readFileSync(`/proc/${descendant}/stat`, 'utf8'));
        }
      } catch (error) {
        if (['ESRCH', 'ENOENT'].includes(error.code)) alive = false;
        else throw error;
      }
      if (alive) await new Promise((resolve) => setTimeout(resolve, 10));
    }
    assert.equal(alive, false, 'The diagnostic descendant must not survive its parent');
  } finally {
    if (descendant) {
      try { process.kill(descendant, 'SIGKILL'); } catch { /* Already terminated. */ }
    }
  }
});

test('missing executable is diagnostic data, not an unhandled process error', async () => {
  const result = await runProcess('openbitfun-nonexistent-diagnostic-command', []);
  assert.notEqual(result.code, 0);
  assert.match(result.error, /ENOENT/);
});
