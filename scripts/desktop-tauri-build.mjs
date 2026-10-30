#!/usr/bin/env node
/** Runs `tauri build` from src/apps/desktop with CI=true. */
import { spawnSync } from 'child_process';
import { fileURLToPath } from 'url';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'path';
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from 'fs';
import { ensureFlashgrepBinary } from './prepare-flashgrep-resource.mjs';
import { prepareOcrBundle, OCR_TESSDATA_DIR, OCR_LIB_DIR } from './prepare-ocr-bundle.mjs';
import { prepareMiniAppShell } from './prepare-miniapp-shell.mjs';
import { extractProductConfigArg } from './product-customization/cli.mjs';
import { productBuildEnvironment } from './product-customization/projections.mjs';
import { resolveProductDefinition } from './product-customization/resolver.mjs';
import { resolveReleaseChannel } from './release-channel.mjs';
import { requireModernIconTools, verifyMacosIconBundle } from './macos-icon-bundle.mjs';
import {
  WEB_FONT_PROFILE_ENV,
  fontProfileForDesktopTarget,
} from './web-font-profile.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');

function tauriBuildArgsFromArgv() {
  const args = process.argv.slice(2);
  // `node script.mjs -- --foo` leaves a leading `--`; strip so `tauri build` sees the same argv as before.
  let i = 0;
  while (i < args.length && args[i] === '--') {
    i += 1;
  }
  return args.slice(i);
}

async function main() {
  const { productConfig, forwardArgs: forward } = extractProductConfigArg(tauriBuildArgsFromArgv());
  const bundleOnly = forward.includes('--bundle-only');
  if (bundleOnly) forward.splice(forward.indexOf('--bundle-only'), 1);
  const resolution = resolveProductDefinition({ rootDir: ROOT, productConfig, member: 'desktop' });
  Object.assign(process.env, productBuildEnvironment(resolution));
  console.log(`[product] ${resolution.assembly.member} ${resolution.assembly.assemblyDigest}`);
  const fontProfile = configureDesktopWebFontProfile(forward);
  console.log(`[font-profile] ${fontProfile}`);
  const releaseChannel = resolveReleaseChannel(process.env.OPENBITFUN_RELEASE_CHANNEL);
  console.log(`[release] channel=${releaseChannel.channel}`);

  const desktopDir = join(ROOT, 'src', 'apps', 'desktop');
  const macosBundle = requestsMacosBundle(forward);
  if (macosBundle) requireModernIconTools();
  if (!bundleOnly) preparePluginHost();
  const miniappShell = prepareMiniAppShell({
    target: optionValue(forward, '--target'),
    profile: forward.includes('--debug') ? 'debug' : optionValue(forward, '--profile') || 'release',
  });
  const flashgrepBinary = prepareMacOSFlashgrepForSigning(
    ensureFlashgrepBinary({ target: optionValue(forward, '--target') || rustHostTargetTriple() }),
    desktopDir,
  );
  process.env.FLASHGREP_DAEMON_BIN = flashgrepBinary;
  configureLinuxOcrBundle(forward, resolution);
  // Tauri CLI reads CI and rejects numeric "1" (common in CI providers).
  process.env.CI = 'true';
  if (process.platform === 'darwin' && requestsDmgBundle(forward)) {
    // Tauri otherwise passes --skip-jenkins under CI, which drops the branded
    // Finder background and icon positions from the generated DMG.
    process.env.TAURI_BUNDLER_DMG_IGNORE_CI = 'true';
  }

  const tauriConfig = prepareTauriConfig(join(desktopDir, 'tauri.conf.json'), {
    desktopDir,
    flashgrepBinary,
    miniappShell,
    resolution,
    releaseChannel,
  });
  const tauriBin = join(ROOT, 'node_modules', '.bin', 'tauri');
  const tauriArgs = [bundleOnly ? 'bundle' : 'build', '--config', tauriConfig, ...forward];
  let attemptStartedAtMs = Date.now();
  let r = runTauriBuild(tauriBin, tauriArgs, desktopDir);

  const maxMacDmgBuildAttempts = 3;
  for (
    let attempt = 1;
    attempt < maxMacDmgBuildAttempts
      && !r.error
      && shouldRetryMacDmgBuild(r, forward, desktopDir, attemptStartedAtMs);
    attempt += 1
  ) {
    const retryDelaySeconds = attempt * 10;
    console.warn(
      `[tauri-build] DMG bundling failed after the macOS app bundle was refreshed; retrying build attempt ${attempt + 1}/${maxMacDmgBuildAttempts} in ${retryDelaySeconds} seconds.`
    );
    await new Promise((resolveRetry) => setTimeout(resolveRetry, retryDelaySeconds * 1_000));
    attemptStartedAtMs = Date.now();
    r = runTauriBuild(tauriBin, tauriArgs, desktopDir);
  }

  if (r.error) {
    console.error(r.error);
    process.exit(1);
  }

  if (r.status === 0 && macosBundle) {
    const config = JSON.parse(readFileSync(tauriConfig, 'utf8'));
    const targetDir = process.env.CARGO_TARGET_DIR
      ? resolve(desktopDir, process.env.CARGO_TARGET_DIR) : join(ROOT, 'target');
    const target = optionValue(forward, '--target');
    const profile = forward.includes('--debug') ? 'debug' : optionValue(forward, '--profile') || 'release';
    verifyMacosIconBundle(join(targetDir, ...(target ? [target] : []), profile,
      'bundle', 'macos', `${config.productName}.app`));
  }

  // Keep only the latest useful Cargo caches for this build profile after tauri build ends.
  try {
    const { profileFromTauriBuildArgs, runGcBestEffort, targetFromTauriBuildArgs } = await import(
      './cargo-target-gc.mjs'
    );
    runGcBestEffort({
      rootDir: ROOT,
      profile: profileFromTauriBuildArgs(forward),
      triple: targetFromTauriBuildArgs(forward),
    });
  } catch (error) {
    console.warn(`[target-gc] skipped: ${error.message || String(error)}`);
  }

  if (r.status === 0 && forward.includes('--no-bundle')) {
    console.warn(
      '[tauri-build] No bundle was produced. The raw desktop executable depends on its adjacent frontend, flashgrep, mobile-web, and resources directories and must not be distributed by itself.'
    );
  }

  process.exit(r.status ?? 1);
}

export function requestsMacosBundle(args, platform = process.platform) {
  if (args.includes('--no-bundle')) return false;
  const target = optionValue(args, '--target');
  return target ? target.endsWith('-apple-darwin') : platform === 'darwin';
}

function rustHostTargetTriple() {
  const result = spawnSync('rustc', ['-vV'], {
    cwd: ROOT,
    encoding: 'utf8',
    shell: false,
    windowsHide: true,
  });
  if (result.error || result.status !== 0) {
    const detail = result.error?.message || result.stderr || `exit status ${result.status}`;
    throw new Error(`Could not determine the Rust host target: ${detail}`);
  }
  const host = String(result.stdout).match(/^host:\s*(\S+)$/m)?.[1];
  if (!host) throw new Error('rustc -vV did not report a host target triple.');
  return host;
}

function preparePluginHost() {
  const result = spawnSync('pnpm', ['run', 'plugin-host:prepare'], {
    cwd: ROOT,
    env: process.env,
    stdio: 'inherit',
    shell: true,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`OpenCode extension Host preparation failed with exit code ${result.status}`);
  }
}

function runTauriBuild(tauriBin, args, desktopDir) {
  return spawnSync(tauriBin, args, {
    cwd: desktopDir,
    env: process.env,
    stdio: 'inherit',
    shell: true,
  });
}

export function shouldRetryMacDmgBuild(
  result,
  args,
  desktopDir,
  buildStartedAtMs,
  runtime = {}
) {
  const platform = runtime.platform ?? process.platform;
  const githubActions = runtime.githubActions ?? process.env.GITHUB_ACTIONS;
  if (
    result.status === 0 ||
    platform !== 'darwin' ||
    githubActions !== 'true' ||
    args.includes('--no-bundle') ||
    !requestsDmgBundle(args)
  ) {
    return false;
  }

  const configuredTargetDir = runtime.cargoTargetDir ?? process.env.CARGO_TARGET_DIR;
  const targetDir = configuredTargetDir
    ? isAbsolute(configuredTargetDir)
      ? configuredTargetDir
      : resolve(desktopDir, configuredTargetDir)
    : join(runtime.root ?? ROOT, 'target');
  const target = optionValue(args, '--target');
  const profile = args.includes('--debug') ? 'debug' : optionValue(args, '--profile') || 'release';
  const bundleDir = join(
    targetDir,
    ...(target ? [target] : []),
    profile,
    'bundle',
    'macos'
  );

  const freshAfterMs = buildStartedAtMs - 1_000;
  try {
    return readdirSync(bundleDir, { withFileTypes: true }).some((entry) => {
      if (!entry.isDirectory() || !entry.name.endsWith('.app')) {
        return false;
      }

      const appDir = join(bundleDir, entry.name);
      if (statSync(appDir).mtimeMs >= freshAfterMs) {
        return true;
      }

      // The Rust cache can restore an existing app directory without changing
      // its own mtime. Tauri still refreshes the executable inside it before
      // codesigning, so use that file as the reliable bundling boundary.
      const executableDir = join(appDir, 'Contents', 'MacOS');
      return readdirSync(executableDir, { withFileTypes: true }).some(
        (executable) =>
          executable.isFile()
          && statSync(join(executableDir, executable.name)).mtimeMs >= freshAfterMs
      );
    });
  } catch {
    return false;
  }
}

function requestsDmgBundle(args) {
  const bundles = optionValue(args, '--bundles');
  return bundles === undefined || bundles.split(',').some((bundle) => bundle.trim() === 'dmg');
}

function optionValue(args, option) {
  const inlinePrefix = `${option}=`;
  for (let i = 0; i < args.length; i += 1) {
    if (args[i] === option) {
      return args[i + 1];
    }
    if (args[i].startsWith(inlinePrefix)) {
      return args[i].slice(inlinePrefix.length);
    }
  }
  return undefined;
}

export function configureDesktopWebFontProfile(
  args,
  { env = process.env, platform = process.platform } = {},
) {
  const profile = fontProfileForDesktopTarget({
    target: optionValue(args, '--target'),
    platform,
  });
  env[WEB_FONT_PROFILE_ENV] = profile;
  return profile;
}

export function prepareMacOSFlashgrepForSigning(
  flashgrepBinary,
  desktopDir,
  runtime = {},
) {
  const platform = runtime.platform ?? process.platform;
  const signingIdentity = runtime.signingIdentity ?? process.env.APPLE_SIGNING_IDENTITY;
  if (platform !== 'darwin' || !signingIdentity) {
    return flashgrepBinary;
  }

  const signedDir = join(desktopDir, 'gen', 'signed-resources', 'flashgrep');
  const signedBinary = join(signedDir, basename(flashgrepBinary));
  mkdirSync(signedDir, { recursive: true });
  copyFileSync(flashgrepBinary, signedBinary);
  chmodSync(signedBinary, statSync(signedBinary).mode | 0o111);

  const run = runtime.spawnSync ?? spawnSync;
  const result = run(
    'codesign',
    [
      '--force',
      '--sign',
      signingIdentity,
      '--options',
      'runtime',
      '--timestamp',
      signedBinary,
    ],
    { encoding: 'utf8', shell: false, windowsHide: true },
  );
  if (result.error || result.status !== 0) {
    const detail = result.error?.message || result.stderr || `exit status ${result.status}`;
    throw new Error(`Failed to sign bundled flashgrep binary: ${detail}`);
  }

  console.log(`[tauri-build] Signed bundled flashgrep binary: ${signedBinary}`);
  return signedBinary;
}

// The cloud private key remains in SimplySign; only its certificate selector is
// passed to Tauri. Authenticode runs before Tauri creates updater signatures.
export function configureWindowsSigning(config, env = process.env, platform = process.platform) {
  if (platform !== 'win32' || !env.WINDOWS_CERTIFICATE_THUMBPRINT) return;
  const thumbprint = env.WINDOWS_CERTIFICATE_THUMBPRINT.replace(/\s/g, '').toUpperCase();
  if (!/^[0-9A-F]{40}$/.test(thumbprint)) {
    throw new Error('WINDOWS_CERTIFICATE_THUMBPRINT must be a SHA-1 certificate fingerprint.');
  }
  config.bundle ??= {};
  config.bundle.windows = {
    ...config.bundle.windows,
    certificateThumbprint: thumbprint,
    digestAlgorithm: 'sha256',
    timestampUrl: 'http://time.certum.pl',
    tsp: true,
  };
}

export function prepareTauriConfig(
  baseConfigPath,
  { desktopDir, flashgrepBinary, miniappShell, resolution, releaseChannel }
) {
  const config = JSON.parse(readFileSync(baseConfigPath, 'utf8'));
  if (resolution) {
    const productName =
      resolution.productNames[resolution.assembly.fallbackLocale]
      ?? resolution.productNames[resolution.assembly.defaultLocale];
    config.productName = productName;
    config.mainBinaryName = resolution.assembly.binaryName;
    config.identifier = resolution.assembly.bundleId;
  }
  // Let Tauri merge platform icon lists. Re-emitting the base list here would
  // overwrite tauri.macos.conf.json and silently discard the .icon source.
  delete config.bundle.icon;
  configureWindowsSigning(config);
  if (miniappShell) {
    config.bundle.externalBin = [...(config.bundle.externalBin || []),
      toTauriPath(relative(desktopDir, miniappShell.sidecarBase))];
  }
  injectTargetFlashgrepResource(config, desktopDir, flashgrepBinary);
  // The DeepSeek bridge is not a compile-time resource: cargo check and
  // desktop:dev must not require packages/dsh-acp/dist-profile. Official
  // packaging injects it here; frontend:build-all (beforeBuildCommand)
  // compiles the profile before Tauri copies resources.
  injectDshProfileResource(config);
  injectExternalFrontendResource(config);
  injectLinuxOcrResources(config, desktopDir);

  const release = releaseChannel
    ?? resolveReleaseChannel(process.env.OPENBITFUN_RELEASE_CHANNEL);
  const primaryEndpoint =
    process.env.TAURI_UPDATER_ENDPOINT || release.primaryUpdaterEndpoint;
  const fallbackEndpoint =
    process.env.TAURI_UPDATER_FALLBACK_ENDPOINT || release.fallbackUpdaterEndpoint;
  process.env.OPENBITFUN_RELEASE_CHANNEL = release.channel;
  process.env.OPENBITFUN_UPDATER_PRIMARY_ENDPOINT = primaryEndpoint;
  process.env.OPENBITFUN_UPDATER_FALLBACK_ENDPOINT = fallbackEndpoint;

  const enabled = ['1', 'true', 'yes'].includes(
    String(process.env.OPENBITFUN_ENABLE_UPDATER_ARTIFACTS || '').toLowerCase()
  );

  if (enabled) {
    const pubkey = process.env.TAURI_UPDATER_PUBKEY;
    if (!pubkey) {
      console.error('OPENBITFUN_ENABLE_UPDATER_ARTIFACTS is set, but TAURI_UPDATER_PUBKEY is missing.');
      process.exit(1);
    }
    if (!process.env.TAURI_SIGNING_PRIVATE_KEY) {
      console.error('OPENBITFUN_ENABLE_UPDATER_ARTIFACTS is set, but TAURI_SIGNING_PRIVATE_KEY is missing.');
      process.exit(1);
    }

    // Fallback endpoint used when GitHub is unreachable (not when no update is found).
    // Tauri updater iterates endpoints and only falls through on network/HTTP errors;
    // a 204 (no update) or a successfully parsed manifest stops the loop.
    config.bundle = {
      ...(config.bundle || {}),
      createUpdaterArtifacts: true,
    };
    config.plugins = {
      ...(config.plugins || {}),
      updater: {
        endpoints: [primaryEndpoint, fallbackEndpoint],
        pubkey,
        windows: {
          installMode: 'quiet',
        },
      },
    };
    console.log(
      `[tauri-build] Updater artifacts enabled for ${release.channel}: ${primaryEndpoint} (fallback: ${fallbackEndpoint})`
    );
  }

  const generatedDir = join(desktopDir, 'gen');
  mkdirSync(generatedDir, { recursive: true });
  const generatedConfig = join(
    generatedDir,
    resolution
      ? `tauri.${resolution.assembly.assemblyDigest}.generated.conf.json`
      : 'tauri.generated.conf.json',
  );
  writeFileSync(generatedConfig, `${JSON.stringify(config, null, 2)}\n`, 'utf8');
  return generatedConfig;
}

const DSH_PROFILE_RESOURCE_SOURCE = '../../../packages/dsh-acp/dist-profile';
const DSH_PROFILE_RESOURCE_TARGET = 'resources/dsh-profile';
const EXTERNAL_FRONTEND_RESOURCE_SOURCE = '../../../dist';
const EXTERNAL_FRONTEND_RESOURCE_TARGET = 'frontend/dist';

function injectDshProfileResource(config) {
  const resources = { ...(config.bundle?.resources || {}) };
  resources[DSH_PROFILE_RESOURCE_SOURCE] = DSH_PROFILE_RESOURCE_TARGET;
  config.bundle = {
    ...(config.bundle || {}),
    resources,
  };
}

function injectExternalFrontendResource(config) {
  const resources = { ...(config.bundle?.resources || {}) };
  resources[EXTERNAL_FRONTEND_RESOURCE_SOURCE] = EXTERNAL_FRONTEND_RESOURCE_TARGET;
  config.bundle = {
    ...(config.bundle || {}),
    resources,
  };
}

const OCR_TESSDATA_RESOURCE_TARGET = 'resources/ocr/tessdata';
const OCR_LIB_RESOURCE_TARGET = 'resources/ocr/lib';

// Linux OCR is delivered self-contained: the language traineddata and the
// tesseract/leptonica shared-library closure are staged beside the app on the
// build host and shipped as bundle resources. This runs only for Linux targets
// and is a no-op elsewhere (macOS/Windows use the OS OCR backends).
export function configureLinuxOcrBundle(
  args,
  resolution,
  { env = process.env, platform = process.platform, prepare = prepareOcrBundle } = {},
) {
  const target = optionValue(args, '--target');
  const isLinux = target ? target.includes('linux') : platform === 'linux';
  if (!isLinux || args.includes('--no-bundle')) return null;
  const manifest = prepare({ target });
  if (!manifest || !manifest.staged) return manifest;

  // The packaged binary installs to /usr/bin/<bin> and its resources to
  // /usr/lib/<bin>/resources, so an $ORIGIN-relative RUNPATH points at the
  // bundled libraries without depending on the target host's SONAME.
  const binaryName = resolution?.assembly?.binaryName ?? 'openbitfun-desktop';
  const rpath = `$ORIGIN/../lib/${binaryName}/resources/ocr/lib`;
  const linkArg = `-C link-args=-Wl,--enable-new-dtags,-rpath,${rpath}`;
  const existing = env.RUSTFLAGS ? `${env.RUSTFLAGS} ` : '';
  if (!env.RUSTFLAGS?.includes(rpath)) {
    env.RUSTFLAGS = `${existing}${linkArg}`.trim();
  }
  console.log(`[tauri-build] Linux OCR self-containment: RUNPATH=${rpath}`);
  return manifest;
}

// Add the staged OCR resource directories, when present, so Tauri copies them
// into every Linux bundle. Guarded on directory existence, which only happens
// after a Linux staging pass, so macOS/Windows configs stay unchanged.
function injectLinuxOcrResources(config, desktopDir) {
  if (!existsSync(OCR_TESSDATA_DIR) && !existsSync(OCR_LIB_DIR)) return;
  const resources = { ...(config.bundle?.resources || {}) };
  if (existsSync(OCR_TESSDATA_DIR)) {
    resources[toTauriPath(relative(desktopDir, OCR_TESSDATA_DIR))] = OCR_TESSDATA_RESOURCE_TARGET;
  }
  if (existsSync(OCR_LIB_DIR)) {
    resources[toTauriPath(relative(desktopDir, OCR_LIB_DIR))] = OCR_LIB_RESOURCE_TARGET;
  }
  config.bundle = {
    ...(config.bundle || {}),
    resources,
  };
}

function injectTargetFlashgrepResource(config, desktopDir, flashgrepBinary) {
  const resources = { ...(config.bundle?.resources || {}) };
  delete resources['../../../resources/flashgrep'];

  for (const binaryPath of bundledFlashgrepResources(flashgrepBinary)) {
    const source = toTauriPath(relative(desktopDir, binaryPath));
    resources[source] = `flashgrep/${basename(binaryPath)}`;
  }
  config.bundle = {
    ...(config.bundle || {}),
    resources,
  };
}

function bundledFlashgrepResources(primaryBinary) {
  return primaryBinary ? [primaryBinary] : [];
}

function toTauriPath(value) {
  return value.split(sep).join('/');
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((e) => {
    console.error(e);
    process.exit(1);
  });
}
