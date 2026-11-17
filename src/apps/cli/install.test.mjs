import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

test('Bash fallbacks use the same kernel lock and never fall back to shlock', { skip: process.platform !== 'linux' }, t => {
  const temp = mkdtempSync(join(tmpdir(), 'openbitfun-native-lock-'));
  t.after(() => rmSync(temp, { recursive: true, force: true }));
  const source = readFileSync(new URL('./install.sh', import.meta.url), 'utf8').split('for arg in "$@"; do')[0];
  const script = source + `
fixture="$OPENBITFUN_INSTALL_TEST_ROOT"
mkdir -p "$fixture/host"
printf host > "$fixture/host/extension-host.js"
cat > "$fixture/source" <<'EXE'
#!/usr/bin/env bash
if [ "$1" = --internal-hold-installation-lock ]; then
  exec /usr/bin/python3 -c 'import fcntl,sys; lock=open(sys.argv[1],"a+"); fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB); print("OPENBITFUN_INSTALL_LOCK_V1",flush=True); sys.stdin.buffer.read()' "$2"
fi
echo new
EXE
chmod +x "$fixture/source"
shlock() { echo 'Incorrect PID-file lock fallback' >&2; return 97; }
for BACKEND in perl python native; do
  command() {
    if [ "$1" = -v ]; then
      case "$2" in
        flock) return 1;;
        perl) [ "$BACKEND" = perl ] || return 1;;
        python3) [ "$BACKEND" != native ] || return 1;;
      esac
    fi
    builtin command "$@"
  }
  destination="$fixture/$BACKEND"
  mkdir -p "$destination"
  printf old > "$destination/openbitfun"
  exec 8<>"$destination/.openbitfun-cli-install.lock"
  flock -n 8
  inode=$(stat -c '%i' "$destination/.openbitfun-cli-install.lock")
  if install_bundle "$fixture/source" "$fixture/host" "$destination"; then exit 31; fi
  [ "$(cat "$destination/openbitfun")" = old ]
  [ "$(stat -c '%i' "$destination/.openbitfun-cli-install.lock")" = "$inode" ]
  exec 8>&-
  install_bundle "$fixture/source" "$fixture/host" "$destination"
  [ "$("$destination/openbitfun" --version)" = new ]
  [ "$(stat -c '%i' "$destination/.openbitfun-cli-install.lock")" = "$inode" ]
  mktemp() {
    case "$*" in *'/.openbitfun-install.'*) return 1;; esac
    builtin command mktemp "$@"
  }
  install() {
    touch "$OPENBITFUN_INSTALL_TEST_ROOT/unexpected-stage"
    return 97
  }
  if install_bundle "$fixture/source" "$fixture/host" "$destination"; then exit 32; fi
  [ ! -e "$fixture/unexpected-stage" ]
  [ "$("$destination/openbitfun" --version)" = new ]
  unset -f install mktemp
done
echo 'PASS native lock fallback fixtures'
`;
  const file = join(temp, 'fixture.sh'); writeFileSync(file, script);
  const result = spawnSync('bash', [file], { encoding: 'utf8', timeout: 30000,
    env: { ...process.env, OPENBITFUN_INSTALL_TEST_ROOT: temp } });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /PASS native lock fallback fixtures/);
});

test('CLI installer serializes writers, rolls back publication errors and preserves failed recovery copies', t => {
  const temp = mkdtempSync(join(tmpdir(), 'openbitfun-cli-install-test-'));
  t.after(() => rmSync(temp, { recursive: true, force: true }));
  let result;
  if (process.platform === 'win32') {
    const source = readFileSync(new URL('./install.ps1', import.meta.url), 'utf8').split('$repoRoot = Resolve-RepoRoot')[0];
    const script = source + `
$fixture = $env:OPENBITFUN_INSTALL_TEST_ROOT
$source = Join-Path $fixture 'source.exe'
$hostSource = Join-Path $fixture 'host'
[IO.Directory]::CreateDirectory($hostSource) | Out-Null
[IO.File]::WriteAllText($source, 'new')
[IO.File]::WriteAllText((Join-Path $hostSource 'extension-host.js'), 'new host')
$script:failPublished = $false
$script:failRestore = $false
function Assert-Entrypoint([string]$Executable) {
  if ([IO.File]::ReadAllText($Executable) -ne 'new') { throw 'Invalid staged binary' }
  if ($script:failPublished -and [IO.Path]::GetDirectoryName($Executable) -eq $script:destination) { throw 'Injected final verification failure' }
}
function Move-Item {
  param([string]$LiteralPath, [string]$Destination, [switch]$Force)
  if ($script:failRestore -and [IO.Path]::GetFileName($LiteralPath) -eq 'previous-openbitfun.exe') { throw 'Injected recovery failure' }
  Microsoft.PowerShell.Management\\Move-Item @PSBoundParameters
}
function Prepare([string]$name) {
  $script:destination = Join-Path $fixture $name
  [IO.Directory]::CreateDirectory((Join-Path $script:destination 'resources/ext-host')) | Out-Null
  [IO.File]::WriteAllText((Join-Path $script:destination 'openbitfun.exe'), 'old')
  [IO.File]::WriteAllText((Join-Path $script:destination 'resources/ext-host/extension-host.js'), 'old host')
  [IO.File]::WriteAllText((Join-Path $script:destination 'user.txt'), 'keep')
}
function Check([bool]$condition) { if (!$condition) { throw 'Fixture assertion failed' } }
Prepare normal
Install-OpenBitFunCli $source $hostSource $destination
Check ([IO.File]::ReadAllText((Join-Path $destination 'openbitfun.exe')) -eq 'new')
Check ([IO.File]::ReadAllText((Join-Path $destination 'user.txt')) -eq 'keep')
Prepare locked
$lock = [IO.File]::Open((Join-Path $destination '.openbitfun-cli-install.lock'), 'OpenOrCreate', 'ReadWrite', 'None')
$failed = $false
try { Install-OpenBitFunCli $source $hostSource $destination } catch { $failed = $true } finally { $lock.Dispose() }
Check $failed
Check ([IO.File]::ReadAllText((Join-Path $destination 'openbitfun.exe')) -eq 'old')
Prepare rollback
$script:failPublished = $true
$failed = $false
try { Install-OpenBitFunCli $source $hostSource $destination } catch { $failed = $true }
Check $failed
Check ([IO.File]::ReadAllText((Join-Path $destination 'openbitfun.exe')) -eq 'old')
Check ([IO.File]::ReadAllText((Join-Path $destination 'resources/ext-host/extension-host.js')) -eq 'old host')
Prepare recovery
$script:failRestore = $true
$failed = $false
try { Install-OpenBitFunCli $source $hostSource $destination } catch { $failed = $true }
Check $failed
$backup = @(Get-ChildItem -LiteralPath $destination -Directory -Force | Where-Object Name -Like '.openbitfun-install-*')
Check ($backup.Count -eq 1)
Check ([IO.File]::ReadAllText((Join-Path $backup[0].FullName 'previous-openbitfun.exe')) -eq 'old')
Write-Output 'PASS maintenance fixtures'
`;
    result = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(script, 'utf16le').toString('base64')],
      { encoding: 'utf8', windowsHide: true, timeout: 30000, env: { ...process.env, OPENBITFUN_INSTALL_TEST_ROOT: temp } });
  } else {
    const source = readFileSync(new URL('./install.sh', import.meta.url), 'utf8').split('for arg in "$@"; do')[0];
    const script = source + `
fixture="$OPENBITFUN_INSTALL_TEST_ROOT"
mkdir -p "$fixture/host"
printf '%s' 'new host' > "$fixture/host/extension-host.js"
cat > "$fixture/source" <<'EXE'
#!/usr/bin/env bash
if [ "\${FAIL_PUBLISHED:-0}" = 1 ] && [ "$(dirname "$0")" = "$DESTINATION" ]; then exit 1; fi
echo new
EXE
chmod +x "$fixture/source"
prepare() {
  export DESTINATION="$fixture/$1"
  mkdir -p "$DESTINATION/resources/ext-host"
  printf old > "$DESTINATION/openbitfun"
  printf 'old host' > "$DESTINATION/resources/ext-host/extension-host.js"
  printf keep > "$DESTINATION/user.txt"
}
prepare normal
install_bundle "$fixture/source" "$fixture/host" "$DESTINATION"
[ "$("$DESTINATION/openbitfun" --version)" = new ]
[ "$(cat "$DESTINATION/user.txt")" = keep ]
prepare locked
exec 8>"$DESTINATION/.openbitfun-cli-install.lock"
flock -n 8
if install_bundle "$fixture/source" "$fixture/host" "$DESTINATION"; then exit 11; fi
[ "$(cat "$DESTINATION/openbitfun")" = old ]
exec 8>&-
prepare rollback
export FAIL_PUBLISHED=1
if install_bundle "$fixture/source" "$fixture/host" "$DESTINATION"; then exit 12; fi
[ "$(cat "$DESTINATION/openbitfun")" = old ]
[ "$(cat "$DESTINATION/resources/ext-host/extension-host.js")" = 'old host' ]
prepare recovery
mv() { case "$1" in *previous-openbitfun) return 1;; esac; command mv "$@"; }
if install_bundle "$fixture/source" "$fixture/host" "$DESTINATION"; then exit 13; fi
backup=$(find "$DESTINATION" -name previous-openbitfun -type f)
[ "$(cat "$backup")" = old ]
echo 'PASS maintenance fixtures'
`;
    const file = join(temp, 'fixture.sh'); writeFileSync(file, script);
    result = spawnSync('bash', [file], { encoding: 'utf8', timeout: 30000, env: { ...process.env, OPENBITFUN_INSTALL_TEST_ROOT: temp } });
  }
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /PASS maintenance fixtures/);
});
