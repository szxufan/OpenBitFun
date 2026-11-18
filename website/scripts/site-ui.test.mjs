#!/usr/bin/env node

import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import {
  normalizePlaybookCatalog,
  resolveAdjacentCapabilities,
  resolveRelatedCapabilities,
} from '../src/playbook-data.js';
import { matchingItems, scoreCapability, scoreTextMatch, searchCapabilities } from '../src/search.js';

const execFileAsync = promisify(execFile);
const websiteRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const repositoryRoot = path.resolve(websiteRoot, '..');
const sourceRoot = path.join(websiteRoot, 'src');

await execFileAsync(process.execPath, [path.join(websiteRoot, 'scripts/build.mjs')], {
  cwd: repositoryRoot,
});

const [
  appSource,
  playbookDataSource,
  searchSource,
  stylesSource,
  templateSource,
  builtIndex,
  builtApp,
  releaseSource,
  catalogSource,
] = await Promise.all([
  readFile(path.join(sourceRoot, 'app.js'), 'utf8'),
  readFile(path.join(sourceRoot, 'playbook-data.js'), 'utf8'),
  readFile(path.join(sourceRoot, 'search.js'), 'utf8'),
  readFile(path.join(sourceRoot, 'styles.css'), 'utf8'),
  readFile(path.join(sourceRoot, 'index.html'), 'utf8'),
  readFile(path.join(websiteRoot, 'dist/index.html'), 'utf8'),
  readFile(path.join(websiteRoot, 'dist/assets/app.js'), 'utf8'),
  readFile(path.join(websiteRoot, 'dist/release.json'), 'utf8'),
  readFile(path.join(websiteRoot, 'dist/data/capabilities.json'), 'utf8'),
]);
const catalog = JSON.parse(catalogSource);

function cssBlock(selector) {
  const escaped = selector.replaceAll(/[.*+?^${}()|[\]\\]/gu, '\\$&');
  const match = stylesSource.match(new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, 'u'));
  assert.ok(match, `missing CSS block: ${selector}`);
  return match[1];
}

test('theme contract defaults to system and exposes all three choices', () => {
  assert.match(templateSource, /localStorage\.getItem\('openbitfun-playbook-theme'\)/u);
  assert.match(templateSource, /let theme = 'system'/u);
  assert.match(templateSource, /prefers-color-scheme: dark/u);
  assert.match(appSource, /const THEME_CHOICES = \['system', 'light', 'dark'\]/u);
  assert.match(appSource, /data-theme-choice="\$\{theme\}"/u);
  assert.match(appSource, /addEventListener\('change'/u);
  assert.match(templateSource, /data-openbitfun-design-system-root/u);
  assert.match(templateSource, /root\.dataset\.colorScheme/u);
  assert.match(templateSource, /\/assets\/design-tokens\.css/u);
  assert.match(templateSource, /\/assets\/theme\.css/u);
  assert.match(appSource, /--openbitfun-color-surface-canvas/u);
  assert.match(appSource, /documentElement\.dataset\.colorScheme = resolvedTheme\(\)/u);
});

test('hero title, search, and statistics remain in layout flow without CAD grid or tilts', () => {
  assert.match(appSource, /<h1><span>\$\{escapeHtml\(localized\(hero, 'title'\)\)\}<\/span><em>/u);
  assert.match(appSource, /<div class="hero-actions">[\s\S]*?<form class="search-box"[\s\S]*?<div class="hero-stat"/u);

  const title = cssBlock('.hero h1');
  const actions = cssBlock('.hero-actions');
  const statistics = cssBlock('.hero-stat');
  assert.match(title, /display:\s*grid/u);
  assert.match(title, /gap:/u);
  assert.match(actions, /display:\s*grid/u);
  assert.match(actions, /grid-template-columns:\s*minmax\(0, 1fr\) auto/u);
  assert.doesNotMatch(actions, /position:\s*absolute/u);
  assert.doesNotMatch(statistics, /position:\s*absolute/u);
  assert.doesNotMatch(stylesSource, /rotate\(/u);
  assert.doesNotMatch(stylesSource, /background-size:\s*24px 24px/u);
  assert.match(
    stylesSource,
    /@media \(max-width: 1360px\)[\s\S]*?\.hero-actions\s*\{\s*grid-template-columns:\s*minmax\(0, 1fr\)/u,
  );
});

test('live search preserves the input node and supports IME composition', () => {
  assert.match(appSource, /function updateIndexResults\(\)/u);
  assert.match(appSource, /addEventListener\('compositionstart'/u);
  assert.match(appSource, /addEventListener\('compositionend'/u);
  assert.match(appSource, /event\.isComposing/u);

  const inputBinding = appSource.match(
    /const searchInput = document\.querySelector\('#capability-search'\);([\s\S]*?)document\.querySelectorAll\('\[data-kind\]'/u,
  );
  assert.ok(inputBinding, 'missing index search input binding');
  assert.match(inputBinding[1], /updateIndexResults\(\)/u);
  assert.doesNotMatch(inputBinding[1], /renderIndex\(\)/u);
});

test('catalog search accepts bilingual synonym bundles and exposes honest control classes', () => {
  assert.match(searchSource, /function scoreTextMatch\(query, fields\)/u);
  assert.doesNotMatch(appSource, /tokens\.every\(\(token\) => terms\.includes\(token\)\)/u);
  assert.match(appSource, /data-control-kind="\$\{escapeHtml\(item\.control\.kind\)\}"/u);
  assert.match(appSource, /Direct Agent control/u);
  assert.match(appSource, /capability\.operations\?\.length \|\| capability\.agentControl/u);
  assert.match(stylesSource, /\.control-direct/u);
});

test('search accepts consecutive initials without assembling acronyms from scattered prose', () => {
  assert.ok(scoreTextMatch('gs', ['Global Search']) > 0);
  assert.equal(scoreTextMatch('pet', ['Publish the example after completing checks and tests']), 0);
  assert.equal(scoreTextMatch('mascot', ['Move an app; select controls only when observed, then operate them']), 0);
});

test('Playbook search satisfies the shared cross-surface acceptance corpus', () => {
  const normalized = normalizePlaybookCatalog(catalog);
  for (const acceptance of normalized.searchAcceptance) {
    const results = searchCapabilities(normalized, acceptance.query);
    assert.equal(
      results[0]?.capability.id,
      acceptance.expectedFirstCapabilityId,
      acceptance.id,
    );
    const resultIds = new Set(results.map(({ capability }) => capability.id));
    for (const capabilityId of acceptance.expectedCapabilityIds) {
      assert.ok(resultIds.has(capabilityId), `${acceptance.id} missed ${capabilityId}`);
    }
    if (acceptance.expectedItem) {
      const capability = normalized.capabilities.find(({ id }) =>
        id === acceptance.expectedItem.capabilityId);
      assert.equal(
        matchingItems(capability, acceptance.query)[0]?.id,
        acceptance.expectedItem.itemId,
        `${acceptance.id} item route`,
      );
    }
  }
});

test('playbook normalizer provides human fallbacks and honors additive schema enrichments', () => {
  const withoutPlaybook = normalizePlaybookCatalog({
    ...catalog,
    playbook: undefined,
  });
  assert.ok(withoutPlaybook.playbook.gettingStarted.length >= 4);
  assert.ok(withoutPlaybook.playbook.scenarios.length >= 4);
  assert.ok(withoutPlaybook.playbook.directoryGroups.length >= 8);
  assert.equal(withoutPlaybook.capabilities.length, catalog.capabilities.length);

  // Pedagogical ordering puts core sessions before secondary entries rather than ASCII id sort.
  const featureIds = withoutPlaybook.capabilities
    .filter(({ kind }) => kind === 'feature')
    .map(({ id }) => id);
  assert.ok(featureIds.indexOf('feature.ai-assistant') < featureIds.indexOf('feature.agents'));

  const settingIds = withoutPlaybook.capabilities
    .filter(({ kind }) => kind === 'setting')
    .map(({ id }) => id);
  assert.ok(settingIds.indexOf('setting.ai.models') < settingIds.indexOf('setting.ai.memory'));

  // Every entry receives a concrete UI path instead of the old generic placeholder.
  for (const capability of withoutPlaybook.capabilities) {
    assert.ok(capability.entryPathZh.length > 0);
    assert.ok(capability.entryPathEn.length > 0);
    assert.notEqual(capability.entryPathZh, 'OpenBitFun 中的对应功能入口');
    assert.notEqual(capability.entryPathZh, 'OpenBitFun 设置中的对应页面');
    assert.ok(resolveRelatedCapabilities(withoutPlaybook, capability).length >= 2);
  }

  // Enriched additive fields override fallbacks cleanly and power secondary prose search.
  const enriched = normalizePlaybookCatalog({
    ...catalog,
    playbook: {
      hero: {
        kickerZh: '全新指南',
        kickerEn: 'Fresh Guide',
        titleZh: '定制主标题',
        titleEn: 'Custom Hero Title',
        subtitleZh: '定制副标题',
        subtitleEn: 'Custom Hero Subtitle',
        quickQueriesZh: ['远程开发'],
        quickQueriesEn: ['Remote SSH'],
      },
      gettingStarted: [
        {
          id: 'step-one',
          step: 1,
          titleZh: '第一步连接模型',
          titleEn: 'Step 1 Models',
          descriptionZh: '先配好模型',
          descriptionEn: 'Configure models first',
          capabilityId: 'setting.ai.models',
        },
      ],
      scenarios: [
        {
          id: 'remote-flow',
          titleZh: '云端开发流',
          titleEn: 'Cloud Dev Flow',
          descriptionZh: '直连远端主机',
          descriptionEn: 'Connect to remote hosts',
          capabilityIds: ['feature.remote-workspaces'],
        },
      ],
      directoryGroups: [
        {
          id: 'settings-ai',
          kind: 'setting',
          order: 1,
          titleZh: '核心设置组',
          titleEn: 'Core Settings',
          descriptionZh: '核心设置说明',
          descriptionEn: 'Core settings description',
        },
        {
          id: 'start-workspace',
          kind: 'feature',
          order: 1,
          titleZh: '核心功能组',
          titleEn: 'Core Features',
          descriptionZh: '核心功能说明',
          descriptionEn: 'Core features description',
        },
      ],
    },
    capabilities: catalog.capabilities.map((capability) => (
      capability.id === 'feature.terminal'
        ? {
          ...capability,
          directoryGroupId: 'start-workspace',
          order: 1,
          entryPathZh: '工作区底部面板 → 终端页签',
          entryPathEn: 'Bottom Panel → Terminal Tab',
          shortcut: 'Ctrl+`',
          scenariosZh: ['在服务端容器里排查端口占用与日志'],
          scenariosEn: ['Inspect container logs and port bindings'],
          tipsZh: ['关闭面板不会中断后台正在编译的进程'],
          tipsEn: ['Closing the panel keeps background builds alive'],
          relatedIds: ['setting.application.terminal'],
          items: capability.items.map((item, idx) => (
            idx === 0
              ? {
                ...item,
                descriptionZh: '一键新建独立交互式 Shell 会话',
                descriptionEn: 'Open an isolated interactive shell tab in one click',
              }
              : item
          )),
        }
        : capability
    )),
  });

  // Feature groups must always precede setting groups even when both start at order: 1.
  assert.deepEqual(
    enriched.playbook.directoryGroups.map(({ id }) => id),
    ['start-workspace', 'settings-ai'],
  );
  assert.equal(enriched.capabilities[0].kind, 'feature');
  assert.equal(enriched.playbook.hero.titleZh, '定制主标题');
  assert.equal(enriched.playbook.gettingStarted.length, 1);
  assert.equal(enriched.playbook.scenarios.length, 1);
  const terminal = enriched.capabilities.find(({ id }) => id === 'feature.terminal');
  assert.equal(terminal.entryPathZh, '工作区底部面板 → 终端页签');
  assert.equal(terminal.shortcut, 'Ctrl+`');
  assert.deepEqual(terminal.tipsZh, ['关闭面板不会中断后台正在编译的进程']);
  assert.equal(resolveRelatedCapabilities(enriched, terminal, 1)[0]?.id, 'setting.application.terminal');
  assert.ok(scoreCapability(terminal, '不会中断后台正在编译的进程') > 0);
  assert.equal(matchingItems(terminal, '独立交互式 Shell 会话')[0]?.id, terminal.items[0].id);

  const adjacent = resolveAdjacentCapabilities(enriched, enriched.capabilities[1].id);
  assert.equal(adjacent.previous?.id, enriched.capabilities[0].id);
  assert.equal(adjacent.next?.id, enriched.capabilities[2].id);
});

test('UI avoids bilingual clutter, fixes GitHub link, and keeps Cmd/Ctrl+K in-page with IME guards', () => {
  assert.doesNotMatch(appSource, /function alternate\(/u);
  assert.doesNotMatch(appSource, /GCWing\/OpenBitFun/u);
  assert.match(appSource, /https:\/\/github\.com\/openBitFun\/OpenBitFun/u);
  assert.doesNotMatch(appSource, /location\.href\s*=\s*'\/'/u);
  assert.match(appSource, /id="quick-jump-modal"/u);
  assert.match(appSource, /setQuickJumpOpen\(!state\.quickJumpOpen\)/u);
  assert.match(appSource, /quickComposing \|\| event\.isComposing \|\| event\.keyCode === 229/u);
  assert.match(appSource, /if \(event\.isComposing \|\| event\.keyCode === 229\) return;/u);
  assert.match(appSource, /data-step-count="\$\{stepCount\}"/u);
  assert.doesNotMatch(appSource, /新手起步 · 4 步跑通/u);
  assert.match(appSource, /!localized\(capability, 'entryPath'\)\.includes\(capability\.shortcut\)/u);
  assert.match(appSource, /history\.pushState/u);
  assert.match(appSource, /addEventListener\('popstate'/u);
  // Never dump raw internal LLM system-prompt agentControl.workflow instructions to readers.
  assert.doesNotMatch(appSource, /agentControl,\s*'workflow'/u);
  assert.doesNotMatch(playbookDataSource, /GCWing/u);
  assert.match(stylesSource, /a:focus-visible,\s*button:focus-visible/u);
  assert.match(stylesSource, /::-webkit-search-cancel-button/u);
  assert.match(stylesSource, /@media \(max-width: 340px\)/u);
  assert.doesNotMatch(cssBlock('.detail-aside'), /background:/u);
  assert.match(cssBlock('.detail-aside'), /max-height:\s*calc\(100vh - 108px\)/u);
});

test('sidebar navigation preserves its scroll position across page loads', () => {
  assert.match(appSource, /SIDEBAR_SCROLL_STORAGE_KEY/u);
  assert.match(appSource, /sessionStorage\.setItem/u);
  assert.match(appSource, /data-sidebar-scroll/u);
  assert.match(appSource, /addEventListener\('scroll', saveSidebarScroll/u);
  assert.match(appSource, /link\.addEventListener\('click', saveSidebarScroll\)/u);
  assert.match(appSource, /sidebarNav\.scrollTop = savedScrollTop/u);
  assert.match(appSource, /sidebarNav\.querySelector\('\[aria-current="page"\]'\)/u);
});

test('callouts and ordinary UI consume canonical semantic color tokens only', () => {
  assert.match(cssBlock('.agent-card'), /background:\s*var\(--openbitfun-color-status-success-surface\)/u);
  assert.match(cssBlock('.agent-card'), /color:\s*var\(--openbitfun-color-status-success-content\)/u);
  assert.match(cssBlock('.control-unsupported'), /color:\s*var\(--openbitfun-color-status-danger-content\)/u);
  assert.doesNotMatch(stylesSource, /#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(/iu);
  assert.doesNotMatch(stylesSource, /var\(--(?:paper|ink|muted|line|panel|accent|danger|shadow)\)/u);
});

test('build emits a source-sensitive immutable release id across all modules', () => {
  const release = JSON.parse(releaseSource);
  assert.match(release.releaseId, /^[0-9a-f]{12}$/u);
  assert.equal(release.releaseId, release.assetVersion);
  assert.match(release.catalogDigest, /^[0-9a-f]{64}$/u);
  assert.match(builtIndex, new RegExp(`/assets/styles\\.css\\?v=${release.releaseId}`, 'u'));
  assert.match(builtIndex, new RegExp(`/assets/design-tokens\\.css\\?v=${release.releaseId}`, 'u'));
  assert.match(builtIndex, new RegExp(`/assets/theme\\.css\\?v=${release.releaseId}`, 'u'));
  assert.match(builtIndex, new RegExp(`/assets/app\\.js\\?v=${release.releaseId}`, 'u'));
  assert.match(builtApp, new RegExp(`\\./search\\.js\\?v=${release.releaseId}`, 'u'));
  assert.match(builtApp, new RegExp(`\\./playbook-data\\.js\\?v=${release.releaseId}`, 'u'));
});
