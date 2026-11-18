# CI helper scripts

Keep normal validation gates unchanged when adding failure diagnostics. Do not
change feature sets, increase the gating test stack, or accept a diagnostic
rerun as a substitute for the original result. Child processes must set
`windowsHide: true`.

## Rust stack-overflow diagnostics

`rust-stack-diagnostics.mjs` wraps an existing `cargo test` command. It records
test output and reads Cargo's JSON artifact messages to identify the exact test
executables, features, and profiles. Successful runs and ordinary test failures
do not trigger extra commands. There is no second build, debugger installation,
or change to the default stack/debug profile on the successful CI path.

```bash
node scripts/ci/rust-stack-diagnostics.mjs --output /tmp/rust-stack-diagnostics -- --locked -p openbitfun-core --features product-full --lib
```

Arguments after the first `--` are passed to `cargo test`, including optional
libtest arguments after a second `--`. The wrapper owns `--message-format`.
Use a dedicated output directory per invocation. CI enables it for the core
library suite on Linux, macOS, and Windows, retaining `product-full` only on
Linux. Other native libtest commands can use the same wrapper and upload pattern
without hard-coded package/test names.

When a failing run prints Rust's stack-overflow signature or Windows reports
`STATUS_STACK_OVERFLOW` (`0xc00000fd`):

- Save the original log, Cargo result, artifact metadata, and toolchain version.
- Validate the exact test name against the built binaries' `--list` output.
  Ambiguous names, worker threads, and discovery failures retain evidence and
  stop without claiming an isolated reproduction. A native overflow without a
  named test thread also retains evidence without guessing which test failed.
- Run the one matching test with the original stack setting, under the platform
  debugger if available, and with `RUST_MIN_STACK=16777216` as a comparison.
  The reruns use the owning package directory and reconstructed loader paths.
- Bound all diagnostics to three minutes total: discovery at most 30 seconds,
  debugger at most 60 seconds, and each isolated run at most 30 seconds. POSIX
  timeout cleanup kills the diagnostic process group; Windows uses
  `taskkill /PID <pid> /T /F`, reserving up to five seconds for cleanup. If tree
  cleanup fails, report it explicitly and terminate the direct child.
- Preserve the original Cargo exit code regardless of diagnostic outcomes.

Download `rust-stack-diagnostics-Linux`, `rust-stack-diagnostics-macOS`, or
`rust-stack-diagnostics-Windows` after its upload step completes. Start
with `summary.json` and `cargo-test.log`, then compare `original-stack.log`,
the debugger log, and `large-stack.log`. A successful large-stack run suggests stack
pressure; it does not distinguish recursion from large async state. Zero tests,
timeouts, missing libraries/debugger, and independent rerun errors are not a
successful reproduction. These are isolated reruns, not a capture of the
original process; shared-state/order-dependent failures may not recur.

Platform adapters:

| Host | Debugger | Loader environment | Debugger log |
| --- | --- | --- | --- |
| Linux | GDB from PATH | `LD_LIBRARY_PATH` | `gdb.log` |
| macOS | LLDB from PATH (Xcode command-line tools) | `DYLD_FALLBACK_LIBRARY_PATH`, also set in LLDB's target environment for SIP | `lldb.log` |
| Windows | CDB from PATH or the matching architecture under Windows Kits 10 Debuggers | case-insensitive PATH | `cdb.log` |

CDB is the console component of Debugging Tools for Windows; a WinDbg GUI-only
installation is not enough. Do not launch GUI debuggers or install SDKs in CI.
Debugger absence or launch restrictions are recorded and do not prevent the
large-stack comparison. GDB disables debuginfod; CDB uses local PDB paths without
symbol-server downloads. LLDB uses batch crash commands; CDB stops on stack
overflow, access violation, fast-fail, and fatal application-exit exceptions.

Automatic reruns support native Linux/macOS/Windows libtest targets, not doctests,
custom harnesses, cross-target runners, or custom runner environment injection.
Debuggers use the original debug information; stripped CI builds may yield only
symbols/addresses. No automatic rebuild with debug info is attempted. If more
evidence is needed, reproduce locally with debug info or add a targeted future
layout/checkpoint probe. Logs contain normal test output; never log credentials.

## Focused verification

```bash
node --test scripts/ci/rust-stack-diagnostics.test.mjs
node --test --test-name-pattern='keeps Rust CI independent' scripts/check-github-config.test.mjs
```

For workflow changes also run `pnpm run check:github-config`. Some unrelated
shell fixtures require Linux tools and do not work under Windows Bash bridges.
Script fixtures do not establish live remote-workspace, relay, Peer Device, or
Detached Dispatch behavior.
