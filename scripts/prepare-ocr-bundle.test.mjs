import assert from 'node:assert/strict';
import test from 'node:test';
import { collectLibClosure, parseLddLine } from './prepare-ocr-bundle.mjs';

test('parseLddLine keeps resolved absolute paths only', () => {
  assert.equal(
    parseLddLine('\tlibtesseract.so.5 => /usr/lib/x86_64-linux-gnu/libtesseract.so.5 (0x00007f00)'),
    '/usr/lib/x86_64-linux-gnu/libtesseract.so.5',
  );
  assert.equal(parseLddLine('\tlinux-vdso.so.1 (0x00007ffd)'), null);
  assert.equal(parseLddLine('\tlibfoo.so => not found'), null);
  assert.equal(parseLddLine('statically linked'), null);
});

test('collectLibClosure walks the dependency graph and drops blacklisted system libs', (t) => {
  const graph = {
    '/usr/lib/libtesseract.so.5': [
      '\tleptonica.so.6 => /usr/lib/libleptonica.so.6 (0x1)',
      '\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x2)',
      '\tlibstdc++.so.6 => /usr/lib/libstdc++.so.6 (0x3)',
    ],
    '/usr/lib/libleptonica.so.6': [
      '\tlibjpeg.so.8 => /usr/lib/libjpeg.so.8 (0x4)',
      '\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x5)',
    ],
    '/usr/lib/libjpeg.so.8': ['\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x6)'],
  };
  // existsSync is consulted on resolved deps; stub ldd only and rely on the
  // real fs for the existence guard by pointing at a file that does exist.
  const runLdd = (path) => graph[path] || [];
  // collectLibClosure filters non-existent files; use a path we know exists.
  const realFile = process.execPath;
  graph[realFile] = [];
  const closure = collectLibClosure([realFile, '/usr/lib/libtesseract.so.5'], { runLdd });
  assert.ok(closure.includes(realFile), 'seed file that exists is kept');
  assert.ok(!closure.some((p) => p.includes('libc.so')), 'libc is blacklisted');
  assert.ok(!closure.some((p) => p.includes('libstdc++')), 'libstdc++ is blacklisted');
  t.diagnostic(`closure=${JSON.stringify(closure)}`);
});
