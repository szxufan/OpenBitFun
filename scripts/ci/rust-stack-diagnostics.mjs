import { spawn } from 'node:child_process';
import { appendFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';

// Stream output rather than retaining a full build log in memory. Diagnostic
// POSIX diagnostics use process groups; Windows uses taskkill's process tree.
export function runProcess(command, args, { env = process.env, cwd, timeoutMs,
  onLine = () => {}, } = {}) {
  return new Promise((resolve) => {
    const grouped = Boolean(timeoutMs) && process.platform !== 'win32';
    const child = spawn(command, args, {
      env, cwd, windowsHide: true, detached: grouped, stdio: ['ignore', 'pipe', 'pipe'],
    });
    let timedOut = false;
    let error;
    let cleanup;
    let cleanupError;
    const readers = [child.stdout, child.stderr].map((stream, index) => {
      const reader = createInterface({ input: stream, crlfDelay: Infinity });
      reader.on('line', (line) => onLine(line, index === 0 ? 'stdout' : 'stderr'));
      return reader;
    });
    const timer = timeoutMs && setTimeout(() => {
      timedOut = true;
      try {
        if (grouped) process.kill(-child.pid, 'SIGKILL');
        else if (child.pid) {
          cleanup = new Promise((done) => {
            const killer = spawn(path.win32.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'taskkill.exe'),
              ['/PID', String(child.pid), '/T', '/F'], {
                windowsHide: true, stdio: 'ignore', timeout: 5_000, killSignal: 'SIGKILL',
              });
            killer.on('error', (err) => { cleanupError = err.message; });
            killer.on('close', (code) => {
              if (code !== 0) {
                cleanupError ??= `taskkill exited with ${code}; descendant cleanup could not be confirmed.`;
                child.kill('SIGKILL');
                // Do not hang on inherited pipes if tree cleanup was unavailable.
                child.stdout.destroy();
                child.stderr.destroy();
              }
              done();
            });
          });
        }
      } catch { /* The child may have exited just before the timer. */ }
    }, timeoutMs);
    child.on('error', (err) => { error = err.message; });
    child.on('close', async (code, signal) => {
      clearTimeout(timer);
      readers.forEach((reader) => reader.close());
      await cleanup;
      resolve({ code: code ?? 1, signal, timedOut, ...(error && { error }),
        ...(cleanupError && { cleanupError }) });
    });
  });
}

export function overflowThread(line) {
  return line.match(/thread '([^'\r\n]+)'(?: \(\d+\))? has overflowed its stack/)?.[1];
}

export function loaderEnvironment(platform, env, libraryPaths) {
  const result = { ...env };
  const key = platform === 'win32' ? 'PATH'
    : platform === 'darwin' ? 'DYLD_FALLBACK_LIBRARY_PATH' : 'LD_LIBRARY_PATH';
  const existingKey = platform === 'win32'
    ? Object.keys(env).find((name) => name.toUpperCase() === key) ?? key : key;
  let previous = env[existingKey];
  if (platform === 'darwin' && previous === undefined) {
    previous = [env.HOME && path.posix.join(env.HOME, 'lib'), '/usr/local/lib', '/usr/lib'].filter(Boolean).join(':');
  }
  if (platform === 'win32') {
    // Node sorts duplicate case variants on Windows and passes just one onward.
    for (const name of Object.keys(result)) if (name.toUpperCase() === key) delete result[name];
  }
  result[existingKey] = [...libraryPaths, ...(previous ? [previous] : [])].join(platform === 'win32' ? ';' : ':');
  return result;
}

export function debuggerPlan(platform, executable, testArgs, env, commandFile,
  { fileExists = existsSync, arch = process.arch } = {}) {
  if (platform === 'darwin') {
    return { name: 'lldb', command: 'lldb', env, args: ['--batch', '--no-lldbinit',
      // SIP can strip DYLD_* from LLDB's own environment; set the inferior's
      // loader path explicitly through LLDB's target settings as well.
      '-o', `settings set target.env-vars ${JSON.stringify(`DYLD_FALLBACK_LIBRARY_PATH=${env.DYLD_FALLBACK_LIBRARY_PATH}`)}`,
      '-o', 'run', '-k', 'thread backtrace -c 80 all', '-k', 'register read',
      '-k', 'disassemble --pc --count 16', '--', executable, ...testArgs] };
  }
  if (platform === 'win32') {
    const getEnv = (key) => env[Object.keys(env).find((name) => name.toUpperCase() === key.toUpperCase())];
    const sdkArch = arch === 'ia32' ? 'x86' : arch;
    const candidates = (getEnv('PATH') ?? '').split(';').filter(Boolean)
      .map((dir) => path.win32.join(dir.replace(/^"|"$/g, ''), 'cdb.exe'));
    for (const root of [getEnv('ProgramFiles(x86)'), getEnv('ProgramFiles')].filter(Boolean)) {
      candidates.push(path.win32.join(root, 'Windows Kits', '10', 'Debuggers', sdkArch, 'cdb.exe'));
    }
    // CDB is the console debugger from Debugging Tools for Windows. Never
    // launch the WinDbg GUI or install an SDK as part of a failed CI run.
    const command = candidates.find(fileExists) ?? 'cdb.exe';
    const trace = '.ecxr; ~* kb 0n80; r; u @$ip L10; q';
    return { name: 'cdb', command,
      env: { ...env, _NT_SYMBOL_PATH: '', _NT_ALT_SYMBOL_PATH: '' },
      commands: ['sov', 'av', '0xc0000409', '0x40000015']
        .map((event) => `sxe -c "${trace}" ${event}`).concat('g').join('\n'),
      args: ['-G', '-y', path.win32.dirname(executable), '-cf', commandFile, executable, ...testArgs] };
  }
  return { name: 'gdb', command: 'gdb', env, args: ['--batch', '--nx',
    '-ex', 'set pagination off', '-ex', 'set debuginfod enabled off',
    '-ex', 'handle SIGSEGV stop print nopass', '-ex', 'handle SIGABRT stop print nopass',
    '-ex', 'run', '-ex', 'thread apply all bt 80', '-ex', 'info registers',
    '-ex', 'x/16i $pc', '--args', executable, ...testArgs] };
}

export async function runCargoTests({ cargoArgs, outputDir, env = process.env,
  platform = process.platform, run = runProcess, print = console.log }) {
  // Own Cargo's message format so the executable always comes from this exact
  // feature/profile build, never a stale glob or a second cargo invocation.
  if (cargoArgs.some((arg) => arg.startsWith('--message-format'))) {
    throw new Error('The diagnostic wrapper owns --message-format.');
  }
  outputDir = path.resolve(outputDir);
  mkdirSync(outputDir, { recursive: true });
  const logPath = path.join(outputDir, 'cargo-test.log');
  writeFileSync(logPath, '');
  const artifacts = new Map();
  const libraryPaths = new Set();
  let thread;
  let overflowDetected = false;
  const args = ['test', '--message-format=json-render-diagnostics', ...cargoArgs];
  const original = await run('cargo', args, { env, onLine(line, stream) {
    // Cargo JSON and libtest text share stdout. Ignore only recognized Cargo
    // records; test output that happens to be JSON must remain visible.
    if (stream === 'stdout' && line.startsWith('{')) {
      try {
        const record = JSON.parse(line);
        if (record.reason === 'compiler-artifact') {
          if (record.profile?.test && record.executable && record.manifest_path) {
            artifacts.set(record.executable, record);
          }
          return;
        }
        if (record.reason === 'build-script-executed') {
          for (const entry of record.linked_paths ?? []) {
            libraryPaths.add(entry.replace(/^(native|dependency|crate|framework|all)=/, ''));
          }
          return;
        }
        if (record.reason === 'build-finished') return;
      } catch { /* A test may print a non-JSON line starting with a brace. */ }
    }
    appendFileSync(logPath, `${line}\n`);
    print(line);
    thread = overflowThread(line) ?? thread;
    overflowDetected ||= Boolean(thread) || /fatal runtime error: stack overflow|STATUS_STACK_OVERFLOW|0xc00000fd/i.test(line);
  } });
  if (original.error) print(original.error);
  overflowDetected ||= platform === 'win32' && (original.code >>> 0) === 0xc00000fd;
  if (original.code === 0 || !overflowDetected) return original.code;

  // Write evidence before any diagnostic subprocess: timeouts, missing debuggers,
  // and unsuccessful isolated reproduction must never erase the first crash.
  const report = { command: ['cargo', ...args], original, thread: thread ?? null, platform,
    cwd: process.cwd(), rustMinStack: env.RUST_MIN_STACK ?? null,
    artifacts: [...artifacts.values()], stages: [] };
  const save = () => writeFileSync(path.join(outputDir, 'summary.json'), `${JSON.stringify(report, null, 2)}\n`);
  save();
  if (env.GITHUB_OUTPUT) appendFileSync(env.GITHUB_OUTPUT, 'stack_overflow=true\n');
  const deadline = Date.now() + 180_000;
  async function stage(name, command, stageArgs, options = {}) {
    const lines = [];
    const stagePath = path.join(outputDir, `${name}.log`);
    writeFileSync(stagePath, `${JSON.stringify([command, ...stageArgs])}\n`);
    // Reserve Windows' bounded process-tree cleanup inside the total budget.
    const remaining = deadline - Date.now() - (platform === 'win32' ? 5_000 : 0);
    const result = remaining <= 0 ? { code: 1, timedOut: true } : await run(command, stageArgs, {
      env, ...options, timeoutMs: Math.max(1, Math.min(options.timeoutMs ?? 30_000, remaining)),
      onLine(line) {
        appendFileSync(stagePath, `${line}\n`);
        // Only --list output and toolchain paths need parsing, with a bound.
        if (options.capture && lines.length < 50_000) lines.push(line);
      },
    });
    report.stages.push({ name, ...result });
    save();
    return { ...result, lines };
  }

  try {
    if (!['linux', 'darwin', 'win32'].includes(platform)) {
      report.note = `Automatic stack diagnosis is unsupported on ${platform}.`;
      return original.code;
    }
    await stage('rustc', 'rustc', ['-Vv'], { timeoutMs: 5_000 });
    if (!thread) {
      report.note = 'Stack overflow was reported without a named test thread. Evidence was retained; no isolated rerun was attempted.';
      return original.code;
    }
    const libdir = await stage('rustc-libdir', 'rustc', ['--print', 'target-libdir'],
      { timeoutMs: 5_000, capture: true });
    if (libdir.code === 0 && libdir.lines.length === 1) libraryPaths.add(libdir.lines[0]);
    // Cargo normally extends the dynamic loader path when launching tests.
    // Reuse paths from this build rather than changing features or rebuilding.
    for (const artifact of artifacts.values()) {
      libraryPaths.add(path.dirname(artifact.executable));
      libraryPaths.add(path.dirname(path.dirname(artifact.executable)));
    }
    const diagnosticEnv = loaderEnvironment(platform, env, libraryPaths);
    // Match the complete name, not a substring. Worker/main thread failures
    // cannot safely be mapped to a test; ambiguous multi-target names also stop.
    const candidates = [];
    const listingDeadline = Date.now() + 30_000;
    for (const artifact of artifacts.values()) {
      if (Date.now() >= listingDeadline) {
        report.note = 'Test discovery exceeded its time budget; no isolated rerun was attempted.';
        return original.code;
      }
      const listed = await stage(`list-${report.stages.length}`, artifact.executable,
        [thread, '--exact', '--list'], {
          cwd: path.dirname(artifact.manifest_path), env: diagnosticEnv, capture: true,
          timeoutMs: Math.min(10_000, listingDeadline - Date.now()),
        });
      if (listed.code !== 0) {
        report.note = 'A test target could not be listed; target selection is incomplete.';
        return original.code;
      }
      if (listed.lines.includes(`${thread}: test`)) candidates.push(artifact);
    }
    if (candidates.length !== 1) {
      report.note = `Expected one exact libtest match; found ${candidates.length}. No isolated rerun was attempted.`;
      return original.code;
    }
    const selected = candidates[0];
    report.selectedExecutable = selected.executable;
    const cwd = path.dirname(selected.manifest_path);
    // --include-ignored ensures a previously explicitly selected ignored test
    // really executes, instead of reporting a misleading zero-test success.
    const testArgs = [thread, '--exact', '--include-ignored', '--nocapture', '--test-threads=1'];
    await stage('original-stack', selected.executable, testArgs, { cwd, env: diagnosticEnv });
    const commandFile = path.join(outputDir, 'cdb-commands.txt');
    const debuggerRun = debuggerPlan(platform, selected.executable, testArgs, diagnosticEnv, commandFile);
    if (debuggerRun.commands) writeFileSync(commandFile, `${debuggerRun.commands}\n`);
    report.debugger = debuggerRun.name;
    await stage(debuggerRun.name, debuggerRun.command, debuggerRun.args,
      { cwd, env: debuggerRun.env, timeoutMs: 60_000 });
    await stage('large-stack', selected.executable, testArgs, {
      cwd, env: { ...diagnosticEnv, RUST_MIN_STACK: '16777216' },
    });
    report.note = 'Reruns are diagnostic only. Compare logs; the original Cargo failure remains the gate. Large-stack success suggests stack pressure but does not establish its cause.';
  } catch (error) {
    report.diagnosticError = String(error);
  } finally {
    save();
    print(`Stack-overflow diagnostics saved to ${outputDir}; preserving Cargo exit ${original.code}.`);
  }
  return original.code;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = process.argv.slice(2);
  if (args[0] !== '--output' || !args[1] || args[2] !== '--' || args.length < 4) {
    console.error('Usage: node scripts/ci/rust-stack-diagnostics.mjs --output <directory> -- <cargo test arguments>');
    process.exitCode = 1;
  } else {
    process.exitCode = await runCargoTests({ outputDir: args[1], cargoArgs: args.slice(3) });
  }
}
