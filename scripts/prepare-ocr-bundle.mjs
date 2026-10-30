import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
} from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const DESKTOP_DIR = join(ROOT, 'src', 'apps', 'desktop');
const OCR_RESOURCE_DIR = join(DESKTOP_DIR, 'resources', 'ocr');
export const OCR_TESSDATA_DIR = join(OCR_RESOURCE_DIR, 'tessdata');
export const OCR_LIB_DIR = join(OCR_RESOURCE_DIR, 'lib');

// Language packs OpenBitFun ships OCR data for, matching the product locales
// (English, Simplified Chinese, Traditional Chinese).
export const SUPPORTED_OCR_LANGUAGES = ['eng', 'chi_sim', 'chi_tra'];

// Shared libraries that must resolve from the running system, never from the
// bundled directory: the C library/loader and thread/math primitives have
// stable, backward-compatible ABIs on every Linux target and shadowing them
// with a copied .so is the classic self-contained-package breakage.
const LIB_CLOSURE_BLACKLIST = [
  'linux-vdso',
  'ld-linux',
  'libc.so',
  'libm.so',
  'libdl.so',
  'libpthread.so',
  'librt.so',
  'libresolv.so',
  'libnsl.so',
  'libutil.so',
  'libgcc_s.so',
  'libstdc++.so',
];

// System tessdata locations, most specific first.
const SYSTEM_TESSDATA_DIRS = [
  '/usr/share/tesseract-ocr/5/tessdata',
  '/usr/share/tesseract-ocr/tessdata',
  '/usr/share/tessdata',
];

function isBlacklisted(libPath) {
  const name = basename(libPath);
  return LIB_CLOSURE_BLACKLIST.some((prefix) => name.startsWith(prefix));
}

// Parse a single `ldd` line into an absolute resolved library path, ignoring
// `not found` and non-file rows (linux-vdso, the dynamic loader, ...).
export function parseLddLine(line) {
  const match = line.match(/=>\s*(\/\S+)\s/);
  if (!match) return null;
  const path = match[1];
  return path.includes('not found') ? null : path;
}

// Resolve the transitive shared-library closure of `seedLibs` (each an
// absolute .so path) by running `ldd` recursively, dropping blacklisted and
// already-visited entries. `runLdd` is injectable so the walk can be unit
// tested on any build host without a Linux toolchain.
export function collectLibClosure(seedLibs, { runLdd = ldd, visited = new Set() } = {}) {
  const closure = [];
  const queue = seedLibs.filter((path) => path && !visited.has(path) && !isBlacklisted(path));
  for (const path of queue) visited.add(path);
  for (const path of queue) {
    closure.push(path);
    for (const line of runLdd(path)) {
      const dep = parseLddLine(line);
      if (!dep || visited.has(dep) || isBlacklisted(dep)) continue;
      if (!existsSync(dep)) continue;
      visited.add(dep);
      closure.push(dep);
      for (const nested of collectLibClosure([dep], { runLdd, visited })) {
        if (!closure.includes(nested)) closure.push(nested);
      }
    }
  }
  return [...new Set(closure)];
}

function ldd(libPath) {
  const result = spawnSync('ldd', [libPath], { encoding: 'utf8', windowsHide: true });
  if (result.error || result.status !== 0) return [];
  return result.stdout.split('\n');
}

// Locate the runtime shared libraries tesseract links against on the build
// host, so they can be copied into the package.
function systemSeedLibraries() {
  const seeds = [];
  for (const soname of ['libtesseract.so.5', 'libleptonica.so.6']) {
    const found = searchLibraryPath(soname);
    if (found) seeds.push(found);
  }
  return seeds;
}

function searchLibraryPath(soname) {
  // Prefer the ldconfig cache, which resolves the exact loader-visible path.
  const cache = spawnSync('ldconfig', ['-p'], { encoding: 'utf8', windowsHide: true });
  if (!cache.error && cache.status === 0) {
    const line = cache.stdout.split('\n').find((entry) => entry.includes(soname));
    const match = line?.match(/(\/\S+\.so\S*)/);
    if (match && existsSync(match[1])) return match[1];
  }
  for (const dir of ['/usr/lib/x86_64-linux-gnu', '/usr/lib/aarch64-linux-gnu', '/usr/lib64', '/usr/lib']) {
    const candidate = join(dir, soname);
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

function findSystemTraineddata(language) {
  const fileName = `${language}.traineddata`;
  if (process.env.TESSDATA_PREFIX) {
    for (const base of [process.env.TESSDATA_PREFIX, join(process.env.TESSDATA_PREFIX, 'tessdata')]) {
      const candidate = join(base, fileName);
      if (existsSync(candidate)) return candidate;
    }
  }
  for (const dir of SYSTEM_TESSDATA_DIRS) {
    const candidate = join(dir, fileName);
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

// Copy the supported language traineddata from the build host's tesseract
// packages into the staged resource directory. Languages absent on the host are
// skipped (the runtime degrades to the available subset) but the required
// English pack must be present or the bundle cannot recognize anything.
export function stageOcrTraineddata({ tessdataDir = OCR_TESSDATA_DIR, languages = SUPPORTED_OCR_LANGUAGES, log = console } = {}) {
  mkdirSync(tessdataDir, { recursive: true });
  const staged = [];
  for (const language of languages) {
    const source = findSystemTraineddata(language);
    if (!source) {
      log.warn(`[ocr-bundle] traineddata for '${language}' not found on the build host; skipping (runtime degrades).`);
      continue;
    }
    const destination = join(tessdataDir, `${language}.traineddata`);
    copyFileSync(source, destination);
    staged.push({ language, size: readFileSync(destination).length, sha256: sha256(destination) });
    log.log(`[ocr-bundle] staged traineddata ${language} <- ${source}`);
  }
  if (!staged.some((entry) => entry.language === 'eng')) {
    throw new Error('[ocr-bundle] tesseract-ocr-eng traineddata is required to build a self-contained Linux OCR bundle. Install it on the build host (apt-get install tesseract-ocr-eng).');
  }
  return staged;
}

// Copy the tesseract/leptonica shared-library closure into the staged resource
// directory so the packaged binary no longer depends on the target host
// providing the exact SONAME. Returns the staged library file names.
export function stageOcrLibraries({ libDir = OCR_LIB_DIR, seeds = systemSeedLibraries(), runLdd = ldd, log = console } = {}) {
  mkdirSync(libDir, { recursive: true });
  if (seeds.length === 0) {
    throw new Error('[ocr-bundle] could not locate libtesseract.so.5 / libleptonica.so.6 on the build host. Install libtesseract5 and libleptonica6 (or their -dev packages) before packaging Linux OCR.');
  }
  const closure = collectLibClosure(seeds, { runLdd });
  const staged = [];
  for (const libPath of closure) {
    const destination = join(libDir, basename(libPath));
    copyFileSync(libPath, destination);
    staged.push(basename(libPath));
    log.log(`[ocr-bundle] staged library ${basename(libPath)} <- ${libPath}`);
  }
  return staged;
}

function isLinuxTarget(options) {
  const target = options.target || '';
  return target.includes('linux') || (!target && process.platform === 'linux');
}

// Top-level entry: only acts for Linux build targets. Returns a manifest that
// the build script logs and, if present, the caller wires into bundle resources.
export function prepareOcrBundle(options = {}) {
  if (!isLinuxTarget(options)) return { staged: false, reason: 'non-linux target' };
  const log = options.log || console;
  const traineddata = stageOcrTraineddata({ ...(options.tessdataDir ? { tessdataDir: options.tessdataDir } : {}), log });
  const libraries = stageOcrLibraries({ ...(options.libDir ? { libDir: options.libDir } : {}), log });
  log.log(`[ocr-bundle] Linux OCR self-containment ready: ${traineddata.length} language(s), ${libraries.length} library file(s).`);
  return { staged: true, traineddata, libraries };
}

const listOcrLanguages = () => readdirSync(OCR_TESSDATA_DIR).filter((name) => name.endsWith('.traineddata'));

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.includes('--list-languages')) {
    console.log(existsSync(OCR_TESSDATA_DIR) ? listOcrLanguages().join('\n') : '');
  } else {
    const target = args[args.indexOf('--target') + 1];
    prepareOcrBundle({ target });
  }
}
