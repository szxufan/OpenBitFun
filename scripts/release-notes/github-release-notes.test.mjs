import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';
import {
  compareVersions,
  fallbackNotes,
  parseArgs,
  parseCommitSubject,
  renderGeneratedNotes,
  run,
  selectPreviousTag,
} from './github-release-notes.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const scriptPath = path.join(repoRoot, 'scripts/release-notes/github-release-notes.mjs');

function commitEntries(subjects) {
  return subjects.map((subject, index) => {
    const fullSha = `f${String(index).padStart(39, '0')}`;
    return {
      parsed: parseCommitSubject(subject),
      subject,
      shortSha: fullSha.slice(0, 7),
      fullSha,
    };
  });
}

test('parseArgs accepts the documented flag surface', () => {
  const options = parseArgs(['--version', '1.0.9', '--repo', 'owner/name', '--notes-dir', 'notes']);
  assert.equal(options.version, '1.0.9');
  assert.equal(options.repo, 'owner/name');
  assert.equal(options.notesDir, 'notes');
  assert.equal(parseArgs(['--version=1.0.9']).version, '1.0.9');
  assert.equal(parseArgs(['--help']).help, true);
});

test('parseArgs rejects a missing or non-semver version', () => {
  assert.throws(() => parseArgs([]));
  assert.throws(() => parseArgs(['--version', 'v1.0.9'.replace('v', 'abc')]));
});

test('parseCommitSubject splits type, scope and summary', () => {
  assert.deepEqual(parseCommitSubject('feat(images): add image generation'), {
    type: 'feat',
    scope: 'images',
    summary: 'add image generation',
  });
  assert.deepEqual(parseCommitSubject('fix: preserve conversations'), {
    type: 'fix',
    scope: undefined,
    summary: 'preserve conversations',
  });
  assert.equal(parseCommitSubject('not-conventional commit'), null);
});

test('renderGeneratedNotes groups conventional commits and hides the rest', () => {
  const notes = renderGeneratedNotes(
    '1.0.8',
    'v1.0.7',
    commitEntries([
      'feat(remote-connect): 新增带外 PSK 配对',
      'fix(ai): preserve conversations',
      'perf(flow-chat): cut per-flush cost',
      'refactor(miniapps): trim guidance',
      'docs(miniapps): emphasize app control',
      'ci: workflow tweak',
      'chore(release): bump version',
      'test(agent): add parity coverage',
      'freeform commit without a type',
    ]),
    'owner/name',
  );
  const sections = notes.split('\n').filter((line) => line.startsWith('### '));
  assert.deepEqual(sections, [
    '### Features',
    '### Bug Fixes',
    '### Performance Improvements',
    '### Code Refactoring',
    '### Documentation',
  ]);
  assert.match(notes, /\* \*\*remote-connect:\*\* 新增带外 PSK 配对/);
  assert.match(notes, /\* preserve conversations \(\[f000000\]/);
  assert.ok(!notes.includes('ci:'), 'ci commits stay hidden');
  assert.ok(!notes.includes('chore'), 'chore commits stay hidden');
  assert.ok(!notes.includes('freeform commit'), 'unparsable subjects stay hidden');
  assert.match(
    notes,
    /\*\*Full Changelog\*\*: https:\/\/github\.com\/owner\/name\/compare\/v1\.0\.7\.\.\.v1\.0\.8/,
  );
});

test('renderGeneratedNotes omits commit links without a repository', () => {
  const notes = renderGeneratedNotes('1.0.8', null, commitEntries(['feat(core): add thing']), undefined);
  assert.match(notes, /\* \*\*core:\*\* add thing \(f000000\)/);
  assert.ok(!notes.includes('Full Changelog'), 'compare link needs a previous tag');
});

test('compareVersions orders semver including prereleases', () => {
  const sorted = ['1.0.10', '1.0.3', '1.0.3-beta.1', '1.0.3-beta.2', '1.0.2'].sort(compareVersions);
  assert.deepEqual(sorted, ['1.0.2', '1.0.3-beta.1', '1.0.3-beta.2', '1.0.3', '1.0.10']);
});

test('selectPreviousTag picks the greatest strictly lower tag', () => {
  const tags = ['v1.0.3', 'v1.0.3-beta.1', 'v1.0.8', 'v1.0.10', 'nightly'];
  assert.equal(selectPreviousTag(tags, '1.0.10'), 'v1.0.8');
  assert.equal(selectPreviousTag(tags, '1.0.8'), 'v1.0.3');
  assert.equal(selectPreviousTag(tags, '1.0.3'), 'v1.0.3-beta.1');
  assert.equal(selectPreviousTag(tags, '1.0.3-beta.1'), null);
  assert.equal(selectPreviousTag(tags, '0.9.0'), null);
});

test('fallbackNotes explains the missing note without failing', () => {
  assert.match(fallbackNotes('1.0.9'), /## OpenBitFun 1\.0\.9/);
  assert.match(fallbackNotes('1.0.9'), /commit history/);
});

test('run prefers the hand-written note file for the version', async () => {
  const notesDir = mkdtempSync(path.join(tmpdir(), 'openbitfun-notes-'));
  try {
    writeFileSync(path.join(notesDir, '1.0.9.md'), 'OpenBitFun 1.0.9 桌面端更新\n\n- 手写摘要。\n');
    const notes = await run(['--version', '1.0.9', '--notes-dir', notesDir]);
    assert.match(notes, /手写摘要/);
  } finally {
    rmSync(notesDir, { recursive: true, force: true });
  }
});

test('run generates grouped notes from a real git tag range', () => {
  const repo = mkdtempSync(path.join(tmpdir(), 'openbitfun-notes-git-'));
  const git = (...args) => execFileSync('git', args, { cwd: repo, encoding: 'utf8' });
  try {
    git('init', '-q');
    git('config', 'user.email', 'test@example.com');
    git('config', 'user.name', 'test');
    git('commit', '-q', '--allow-empty', '-m', 'feat(core): seeded feature');
    git('tag', 'v1.0.7');
    git('commit', '-q', '--allow-empty', '-m', 'feat(core): ranged feature');
    git('commit', '-q', '--allow-empty', '-m', 'fix(core): seeded fix');
    git('tag', 'v1.0.8');
    const notes = execFileSync(
      process.execPath,
      [
        scriptPath,
        '--version',
        '1.0.8',
        '--notes-dir',
        path.join(repo, 'missing-notes'),
        '--repo',
        'owner/name',
      ],
      { encoding: 'utf8', cwd: repo },
    );
    assert.match(notes, /### Features/);
    assert.match(notes, /\* \*\*core:\*\* ranged feature/);
    assert.ok(!notes.includes('seeded feature'), 'commits at or below the previous tag stay out');
    assert.match(notes, /### Bug Fixes/);
    assert.match(notes, /compare\/v1\.0\.7\.\.\.v1\.0\.8/);
  } finally {
    rmSync(repo, { recursive: true, force: true });
  }
});

test('run falls back when neither note file nor tags exist', () => {
  const empty = mkdtempSync(path.join(tmpdir(), 'openbitfun-notes-empty-'));
  const git = (...args) => execFileSync('git', args, { cwd: empty, encoding: 'utf8' });
  try {
    git('init', '-q');
    const notes = execFileSync(
      process.execPath,
      [scriptPath, '--version', '1.0.9', '--notes-dir', path.join(empty, 'none')],
      { encoding: 'utf8', cwd: empty },
    );
    assert.match(notes, /## OpenBitFun 1\.0\.9/);
    assert.match(notes, /commit history/);
  } finally {
    rmSync(empty, { recursive: true, force: true });
  }
});

test('the script runs standalone as a CLI', () => {
  const env = { ...process.env };
  delete env.GITHUB_REPOSITORY;
  const output = execFileSync(
    process.execPath,
    [scriptPath, '--version', '1.0.9', '--notes-dir', path.join(repoRoot, 'scripts/release-notes')],
    { encoding: 'utf8', cwd: repoRoot, env },
  );
  assert.match(output, /## OpenBitFun 1\.0\.9/);
});
