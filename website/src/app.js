import {
  normalizePlaybookCatalog,
  resolveAdjacentCapabilities,
  resolveRelatedCapabilities,
} from './playbook-data.js';
import { matchingItems, scoreCapability } from './search.js';

const root = document.querySelector('#app');
const pageData = JSON.parse(document.querySelector('#page-data')?.textContent ?? '{}');
const THEME_STORAGE_KEY = 'openbitfun-playbook-theme';
const LANGUAGE_STORAGE_KEY = 'openbitfun-playbook-language';
const SIDEBAR_SCROLL_STORAGE_KEY = 'openbitfun-playbook-sidebar-scroll';
const THEME_CHOICES = ['system', 'light', 'dark'];

function readPreference(key) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writePreference(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Preference still applies to the current session when storage is unavailable.
  }
}

function readSessionPreference(key) {
  try {
    return sessionStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeSessionPreference(key, value) {
  try {
    sessionStorage.setItem(key, value);
  } catch {
    // Sidebar scroll continuity is a progressive enhancement.
  }
}

function storedTheme() {
  const value = readPreference(THEME_STORAGE_KEY);
  return THEME_CHOICES.includes(value) ? value : 'system';
}

const initialParams = new URLSearchParams(location.search);
const initialKind = initialParams.get('kind');

const state = {
  catalog: null,
  language: readPreference(LANGUAGE_STORAGE_KEY)
    ?? (navigator.language.startsWith('zh') ? 'zh' : 'en'),
  query: initialParams.get('q') ?? '',
  kind: ['all', 'feature', 'setting'].includes(initialKind) ? initialKind : 'all',
  group: initialParams.get('group') ?? 'all',
  theme: storedTheme(),
  quickJumpOpen: false,
  quickJumpQuery: '',
  quickJumpActiveIndex: 0,
};

function escapeHtml(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&apos;');
}

function text(zh, en) {
  return state.language === 'zh' ? zh : en;
}

function localized(value, field) {
  if (!value) return '';
  return value[`${field}${state.language === 'zh' ? 'Zh' : 'En'}`] ?? '';
}

function localizedList(value, field) {
  if (!value) return [];
  const items = value[`${field}${state.language === 'zh' ? 'Zh' : 'En'}`];
  return Array.isArray(items) ? items : [];
}

function normalize(value) {
  return String(value ?? '').toLocaleLowerCase().trim();
}

function kindLabel(kind) {
  return kind === 'feature' ? text('功能指南', 'Feature') : text('设置参考', 'Setting');
}

function kindSectionHeading(kind) {
  return kind === 'feature' ? text('功能指南', 'Feature Guides') : text('设置参考', 'Settings Reference');
}

function themeLabel(theme) {
  if (theme === 'light') return text('浅色', 'Light');
  if (theme === 'dark') return text('深色', 'Dark');
  return text('跟随系统', 'System');
}

function themeIcon(theme) {
  if (theme === 'light') return '☀';
  if (theme === 'dark') return '☾';
  return '◐';
}

function resolvedTheme(theme = state.theme) {
  if (theme !== 'system') return theme;
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

function syncThemeColor() {
  const surface = getComputedStyle(document.documentElement)
    .getPropertyValue('--openbitfun-color-surface-canvas')
    .trim();
  if (surface) {
    document.querySelector('meta[name="theme-color"]')?.setAttribute('content', surface);
  }
}

function syncThemeControls() {
  document.querySelectorAll('[data-theme-choice]').forEach((button) => {
    const active = button.dataset.themeChoice === state.theme;
    button.setAttribute('aria-pressed', String(active));
    button.classList.toggle('active', active);
  });
}

function applyTheme(theme, persist = true) {
  state.theme = THEME_CHOICES.includes(theme) ? theme : 'system';
  document.documentElement.dataset.theme = state.theme;
  document.documentElement.dataset.colorScheme = resolvedTheme();
  syncThemeColor();
  if (persist) writePreference(THEME_STORAGE_KEY, state.theme);
  syncThemeControls();
}

function groupForCapability(capability) {
  return state.catalog.playbook.groupById[capability.directoryGroupId]
    ?? state.catalog.categories[capability.categoryId]
    ?? { titleZh: '', titleEn: '' };
}

function groupTitle(group) {
  return localized(group, 'title');
}

function capabilityHref(capability, focusItemId = '') {
  const suffix = focusItemId
    ? `?focus=${encodeURIComponent(focusItemId)}#item-${encodeURIComponent(focusItemId)}`
    : '';
  return `/capabilities/${encodeURIComponent(capability.id)}/${suffix}`;
}

function activeCapabilityId() {
  if (pageData.kind === 'capability' && pageData.capabilityId) return pageData.capabilityId;
  const match = location.pathname.match(/^\/capabilities\/([^/]+)\/?$/u);
  return match ? decodeURIComponent(match[1]) : '';
}

function themeControl() {
  return `
    <div class="theme-control" role="group" aria-label="${text('颜色主题', 'Color theme')}">
      ${THEME_CHOICES.map((theme) => `
        <button type="button" data-theme-choice="${theme}"
          class="${state.theme === theme ? 'active' : ''}"
          aria-pressed="${state.theme === theme}"
          aria-label="${themeLabel(theme)}" title="${themeLabel(theme)}">
          <span aria-hidden="true">${themeIcon(theme)}</span><span>${themeLabel(theme)}</span>
        </button>`).join('')}
    </div>`;
}

function header() {
  return `
    <header class="site-header">
      <div class="header-start">
        <button class="sidebar-toggle" id="sidebar-toggle" type="button" aria-controls="docs-sidebar" aria-expanded="false">
          <span aria-hidden="true">☰</span><span>${text('目录', 'Contents')}</span>
        </button>
        <a class="brand" href="/" aria-label="${text('OpenBitFun 使用指南首页', 'OpenBitFun Playbook home')}">
          <img class="brand-logo" src="/assets/openbitfun.png" alt="" width="34" height="34" />
          <span class="brand-text">
            <strong>OpenBitFun</strong>
            <small>${text('使用指南 · Playbook', 'User Playbook')}</small>
          </span>
        </a>
      </div>
      <nav class="header-actions" aria-label="${text('全局操作', 'Global actions')}">
        <button class="header-search-trigger" id="quick-jump-trigger" type="button"
          aria-label="${text('快速搜索功能或设置', 'Quick search features and settings')}">
          <span class="trigger-icon" aria-hidden="true">⌕</span>
          <span class="trigger-text">${text('快速跳转…', 'Quick jump…')}</span>
          <kbd>⌘K</kbd>
        </button>
        ${themeControl()}
        <a class="header-link" href="https://openbitfun.com" target="_blank" rel="noopener">${text('官网', 'Website')}</a>
        <a class="header-link" href="https://github.com/szxufan/OpenBitFun" target="_blank" rel="noopener">GitHub</a>
        <button class="language-toggle" id="language-toggle" type="button" aria-label="${text('Switch to English', '切换到中文')}">
          ${state.language === 'zh' ? 'EN' : '中文'}
        </button>
      </nav>
    </header>`;
}

function footer() {
  return `
    <footer class="site-footer">
      <div class="footer-copy">
        <strong>OpenBitFun Playbook</strong>
        <p>${text(
          '本手册与 OpenBitFun 桌面端全局搜索（⌘K）及智能体控制能力实时同源更新——既能自己照着做，也能随时交给 AI 代劳。',
          'Built from the same live capability catalog as OpenBitFun Global Search (⌘K) and Agent Control—follow steps yourself or hand them straight to your agent.',
        )}</p>
      </div>
      <div class="footer-links">
        <a href="https://openbitfun.com" target="_blank" rel="noopener">${text('下载 OpenBitFun', 'Download OpenBitFun')}</a>
        <a href="https://github.com/openBitFun/OpenBitFun" target="_blank" rel="noopener">GitHub</a>
      </div>
    </footer>`;
}

function sidebarSearchText(capability) {
  const group = groupForCapability(capability);
  return escapeHtml(normalize([
    capability.titleZh,
    capability.titleEn,
    group.titleZh,
    group.titleEn,
    ...capability.items.flatMap((item) => [item.titleZh, item.titleEn]),
    ...(capability.searchTerms ?? []),
  ].join(' ')));
}

function sidebarSection(kind) {
  const currentCapabilityId = activeCapabilityId();
  const groups = state.catalog.playbook.directoryGroups
    .filter((group) => group.kind === kind)
    .map((group) => ({
      group,
      entries: state.catalog.capabilities.filter((capability) => capability.directoryGroupId === group.id),
    }))
    .filter(({ entries }) => entries.length > 0);
  const totalCount = groups.reduce((sum, { entries }) => sum + entries.length, 0);

  return `
    <section class="sidebar-section" data-sidebar-section>
      <div class="sidebar-kind-heading">
        <h2>${kindSectionHeading(kind)}</h2>
        <span>${totalCount}</span>
      </div>
      ${groups.map(({ group, entries }) => `
        <div class="sidebar-group" data-sidebar-group>
          <h3>${escapeHtml(groupTitle(group))}</h3>
          <div class="sidebar-links">
            ${entries.map((capability) => `
              <a href="${capabilityHref(capability)}"
                data-sidebar-item data-sidebar-search="${sidebarSearchText(capability)}"
                ${currentCapabilityId === capability.id ? 'class="active" aria-current="page"' : ''}>
                <span>${escapeHtml(localized(capability, 'title'))}</span>
              </a>`).join('')}
          </div>
        </div>`).join('')}
    </section>`;
}

function sidebar() {
  const isHome = !activeCapabilityId();
  return `
    <aside class="docs-sidebar" id="docs-sidebar" aria-label="${text('指南目录', 'Playbook contents')}">
      <div class="sidebar-mobile-head">
        <strong>${text('指南目录', 'Playbook contents')}</strong>
        <button id="sidebar-close" type="button" aria-label="${text('关闭目录', 'Close contents')}">×</button>
      </div>
      <div class="sidebar-search">
        <span aria-hidden="true">⌕</span>
        <input id="sidebar-search" type="search" autocomplete="off" spellcheck="false"
          placeholder="${text('筛选左侧目录…', 'Filter directory…')}" aria-label="${text('筛选指南目录', 'Filter playbook directory')}" />
      </div>
      <nav data-sidebar-scroll>
        <a class="sidebar-home ${isHome ? 'active' : ''}" href="/" ${isHome ? 'aria-current="page"' : ''}>
          <span aria-hidden="true">⌂</span><span>${text('指南首页与总览', 'Home & Overview')}</span>
        </a>
        ${sidebarSection('feature')}
        ${sidebarSection('setting')}
        <p class="sidebar-empty" id="sidebar-empty" hidden>${text('目录中没有匹配项，试试顶部搜索', 'No matching entry in sidebar')}</p>
      </nav>
    </aside>`;
}

function quickJumpResults(query) {
  const trimmed = query.trim();
  if (!trimmed) {
    // Show popular starter entries when input is empty.
    return state.catalog.capabilities.slice(0, 8).map((capability) => ({
      capability,
      matchedItem: null,
    }));
  }
  return state.catalog.capabilities
    .map((capability) => ({
      capability,
      rank: scoreCapability(capability, trimmed),
      matchedItem: matchingItems(capability, trimmed)[0] ?? null,
    }))
    .filter(({ rank }) => rank > 0)
    .sort((left, right) => right.rank - left.rank
      || (left.capability.order ?? 0) - (right.capability.order ?? 0))
    .slice(0, 12);
}

function quickJumpResultsMarkup() {
  const results = quickJumpResults(state.quickJumpQuery);
  if (!results.length) {
    return `
      <div class="quick-jump-empty">
        <p>${text('没有找到对应指南，换个日常词试试（如“终端”“SSH”“主题”或“MCP”）。', 'No guide matched. Try terms like “terminal”, “SSH”, “theme”, or “MCP”.')}</p>
      </div>`;
  }
  return results.map(({ capability, matchedItem }, index) => {
    const group = groupForCapability(capability);
    const href = capabilityHref(capability, matchedItem?.id ?? '');
    const active = index === state.quickJumpActiveIndex;
    return `
      <a class="quick-jump-item ${active ? 'active' : ''}" href="${href}" data-quick-jump-index="${index}">
        <div class="quick-jump-item-main">
          <div class="quick-jump-item-top">
            <span class="kind-badge kind-${escapeHtml(capability.kind)}">${kindLabel(capability.kind)}</span>
            <span class="quick-jump-group">${escapeHtml(groupTitle(group))}</span>
          </div>
          <strong>${escapeHtml(localized(capability, 'title'))}</strong>
          <p>${escapeHtml(matchedItem ? localized(matchedItem, 'title') : localized(capability, 'summary'))}</p>
        </div>
        <span class="quick-jump-arrow" aria-hidden="true">↵</span>
      </a>`;
  }).join('');
}

function quickJumpModalMarkup() {
  const quickQueries = localizedList(state.catalog.playbook.hero, 'quickQueries').slice(0, 6);
  return `
    <div class="quick-jump-modal" id="quick-jump-modal" role="dialog" aria-modal="true"
      aria-label="${text('快速搜索功能与设置', 'Quick search features and settings')}" ${state.quickJumpOpen ? '' : 'hidden'}>
      <div class="quick-jump-backdrop" id="quick-jump-backdrop"></div>
      <div class="quick-jump-panel">
        <div class="quick-jump-input-row">
          <span class="quick-jump-glyph" aria-hidden="true">⌕</span>
          <input id="quick-jump-input" type="search" autocomplete="off" spellcheck="false"
            value="${escapeHtml(state.quickJumpQuery)}"
            placeholder="${text('搜功能、设置或你想做的事…', 'Search features, settings, or tasks…')}" />
          <button type="button" class="quick-jump-esc" id="quick-jump-close">ESC</button>
        </div>
        <div class="quick-jump-chips">
          <span>${text('大家常搜：', 'Popular:')}</span>
          ${quickQueries.map((chip) => `
            <button type="button" class="quick-chip" data-quick-modal-chip="${escapeHtml(chip)}">${escapeHtml(chip)}</button>
          `).join('')}
        </div>
        <div class="quick-jump-list" id="quick-jump-list">
          ${quickJumpResultsMarkup()}
        </div>
      </div>
    </div>`;
}

function pageFrame(content, className = '') {
  return `
    <div class="page-shell ${className}">
      <div class="header-bar">${header()}</div>
      <div class="docs-shell">
        ${sidebar()}
        <div class="docs-content">${content}</div>
      </div>
      <button class="sidebar-backdrop" id="sidebar-backdrop" type="button" aria-label="${text('关闭目录', 'Close contents')}" tabindex="-1"></button>
      ${quickJumpModalMarkup()}
    </div>`;
}

function capabilityCard(capability) {
  const group = groupForCapability(capability);
  const matches = matchingItems(capability, state.query);
  const focus = matches[0]?.id ?? '';
  const href = capabilityHref(capability, focus);
  const highlights = localizedList(capability, 'highlights').slice(0, 2);

  return `
    <a class="capability-card kind-${escapeHtml(capability.kind)}" href="${href}">
      <div class="card-topline">
        <span class="kind-badge kind-${escapeHtml(capability.kind)}">${kindLabel(capability.kind)}</span>
        <span class="category-name">${escapeHtml(groupTitle(group))}</span>
        ${capability.shortcut ? `<kbd class="card-shortcut">${escapeHtml(capability.shortcut)}</kbd>` : ''}
      </div>
      <strong class="card-title">${escapeHtml(localized(capability, 'title'))}</strong>
      <p class="card-summary">${escapeHtml(localized(capability, 'summary'))}</p>
      ${focus
        ? `<div class="matched-item"><i>${text('命中细项', 'Matched')}</i><span>${escapeHtml(localized(matches[0], 'title'))}</span></div>`
        : highlights.length
          ? `<ul class="card-highlights">${highlights.map((point) => `<li>${escapeHtml(point)}</li>`).join('')}</ul>`
          : ''}
      <div class="card-footer">
        <span class="card-entry-hint" title="${escapeHtml(localized(capability, 'entryPath'))}">${escapeHtml(localized(capability, 'entryPath'))}</span>
        <span class="card-link">${text('查看指南', 'Read guide')} <span aria-hidden="true">→</span></span>
      </div>
    </a>`;
}

function visibleGroupsForKind(kind = state.kind) {
  return state.catalog.playbook.directoryGroups.filter(
    (group) => kind === 'all' || group.kind === kind,
  );
}

function filteredCapabilities() {
  const hasQuery = Boolean(normalize(state.query));
  const validGroups = new Set(visibleGroupsForKind().map(({ id }) => id));
  const activeGroup = state.group !== 'all' && validGroups.has(state.group) ? state.group : 'all';

  return state.catalog.capabilities
    .map((capability, index) => ({
      capability,
      index,
      rank: scoreCapability(capability, state.query),
    }))
    .filter(({ capability, rank }) => rank > 0
      && (state.kind === 'all' || capability.kind === state.kind)
      && (activeGroup === 'all' || capability.directoryGroupId === activeGroup))
    .sort((left, right) => {
      if (hasQuery && right.rank !== left.rank) return right.rank - left.rank;
      return left.index - right.index;
    })
    .map(({ capability }) => capability);
}

function emptySearchMarkup() {
  return `
    <div class="empty-state">
      <span class="empty-icon" aria-hidden="true">⌕</span>
      <h3>${text('没有找到匹配的功能或设置', 'No matching feature or setting found')}</h3>
      <p>${text(
        '试试搜日常说法，比如“远程工作区”“配置模型”“终端”“深色模式”或“MCP”。',
        'Try everyday words such as “remote SSH”, “models”, “terminal”, “dark mode”, or “MCP”.',
      )}</p>
      <button type="button" class="reset-filter-button" data-reset-filters>${text('清空筛选条件', 'Clear search & filters')}</button>
    </div>`;
}

function directoryResultsMarkup(capabilities) {
  if (!capabilities.length) return emptySearchMarkup();

  // When actively searching, show relevance-ranked cards immediately in a single grid.
  if (normalize(state.query)) {
    return `<div class="capability-grid">${capabilities.map(capabilityCard).join('')}</div>`;
  }

  // Otherwise organize entries into warm, task-oriented Functional Directory sections.
  const groups = visibleGroupsForKind()
    .map((group) => ({
      group,
      entries: capabilities.filter((capability) => capability.directoryGroupId === group.id),
    }))
    .filter(({ entries }) => entries.length > 0);

  return groups.map(({ group, entries }) => `
    <section class="directory-group-section" id="group-${escapeHtml(group.id)}">
      <header class="directory-group-header">
        <div>
          <div class="directory-group-kicker">
            <span class="kind-badge kind-${escapeHtml(group.kind)}">${kindLabel(group.kind)}</span>
            <span class="group-count">${text(`${entries.length} 篇指南`, `${entries.length} guides`)}</span>
          </div>
          <h3>${escapeHtml(groupTitle(group))}</h3>
          ${localized(group, 'description') ? `<p>${escapeHtml(localized(group, 'description'))}</p>` : ''}
        </div>
      </header>
      <div class="capability-grid">
        ${entries.map(capabilityCard).join('')}
      </div>
    </section>`).join('');
}

function onboardingSectionsMarkup() {
  const { gettingStarted, scenarios } = state.catalog.playbook;
  const byId = new Map(state.catalog.capabilities.map((capability) => [capability.id, capability]));
  const stepCount = gettingStarted.length;
  const zhStepWord = { 2: '两', 3: '三', 4: '四', 5: '五', 6: '六' }[stepCount] ?? `${stepCount} `;
  const enStepWord = { 2: 'two', 3: 'three', 4: 'four', 5: 'five', 6: 'six' }[stepCount] ?? String(stepCount);

  return `
    <div id="home-onboarding" class="home-onboarding" ${
      normalize(state.query) || state.kind !== 'all' || state.group !== 'all' ? 'hidden' : ''
    }>
      ${stepCount ? `
        <section class="onboarding-section" aria-labelledby="quickstart-heading">
          <div class="section-header">
            <div>
              <p class="section-label">${text(`新手起步 · ${stepCount} 步跑通`, `Getting Started · ${stepCount} Steps`)}</p>
              <h2 id="quickstart-heading">${text(`第一次打开 OpenBitFun？从这${zhStepWord}步开始最省心`, `New to OpenBitFun? Start with these ${enStepWord} steps`)}</h2>
            </div>
          </div>
          <div class="quickstart-grid" data-step-count="${stepCount}">
            ${gettingStarted.map((item) => `
              <a class="quickstart-card" href="/capabilities/${encodeURIComponent(item.capabilityId)}/">
                <span class="step-pill">${text(`第 ${item.step} 步`, `Step ${item.step}`)}</span>
                <h3>${escapeHtml(localized(item, 'title'))}</h3>
                <p>${escapeHtml(localized(item, 'description'))}</p>
                <span class="quickstart-cta">${escapeHtml(localized(item, 'actionLabel') || text('查看步骤', 'Open guide'))} <span aria-hidden="true">→</span></span>
              </a>`).join('')}
          </div>
        </section>` : ''}

      ${scenarios.length ? `
        <section class="onboarding-section" aria-labelledby="scenarios-heading">
          <div class="section-header">
            <div>
              <p class="section-label">${text('按场景找功能', 'Real-World Workflows')}</p>
              <h2 id="scenarios-heading">${text('看看大家平时怎么用它解决实际问题', 'Common ways people use OpenBitFun every day')}</h2>
            </div>
          </div>
          <div class="scenario-grid">
            ${scenarios.map((scenario) => {
              const linked = scenario.capabilityIds.map((id) => byId.get(id)).filter(Boolean);
              return `
                <article class="scenario-card">
                  <h3>${escapeHtml(localized(scenario, 'title'))}</h3>
                  <p>${escapeHtml(localized(scenario, 'description'))}</p>
                  <div class="scenario-links">
                    ${linked.map((capability) => `
                      <a class="scenario-pill" href="${capabilityHref(capability)}">
                        <span>${escapeHtml(localized(capability, 'title'))}</span>
                        <span aria-hidden="true">↗</span>
                      </a>`).join('')}
                  </div>
                </article>`;
            }).join('')}
          </div>
        </section>` : ''}
    </div>`;
}

function syncIndexUrl(push = false) {
  const url = new URL(location.href);
  if (state.query) url.searchParams.set('q', state.query);
  else url.searchParams.delete('q');
  if (state.kind !== 'all') url.searchParams.set('kind', state.kind);
  else url.searchParams.delete('kind');
  if (state.group !== 'all') url.searchParams.set('group', state.group);
  else url.searchParams.delete('group');
  if (url.href === location.href) return;
  if (push) history.pushState(null, '', url);
  else history.replaceState(null, '', url);
}

function updateIndexResults() {
  const capabilities = filteredCapabilities();
  const count = document.querySelector('#result-count');
  const resultsContainer = document.querySelector('#capability-grid');
  const onboarding = document.querySelector('#home-onboarding');
  const clearButton = document.querySelector('#clear-search');
  const isFiltered = Boolean(normalize(state.query)) || state.kind !== 'all' || state.group !== 'all';

  if (onboarding) onboarding.hidden = isFiltered;
  if (clearButton) clearButton.hidden = !state.query;
  if (count) {
    count.textContent = normalize(state.query)
      ? text(`找到 ${capabilities.length} 篇相关指南`, `${capabilities.length} matching guides`)
      : text(`共收录 ${capabilities.length} 篇指南`, `Showing ${capabilities.length} guides`);
  }
  if (resultsContainer) {
    resultsContainer.innerHTML = directoryResultsMarkup(capabilities);
    bindResetFilterButtons(resultsContainer);
  }
  document.querySelectorAll('[data-quick-query]').forEach((chip) => {
    chip.classList.toggle('active', normalize(chip.dataset.quickQuery) === normalize(state.query));
  });
}

function bindResetFilterButtons(scope = document) {
  scope.querySelectorAll('[data-reset-filters]').forEach((button) => {
    button.addEventListener('click', () => {
      state.query = '';
      state.kind = 'all';
      state.group = 'all';
      syncIndexUrl(true);
      renderIndex();
    });
  });
}

function renderIndex() {
  const hero = state.catalog.playbook.hero;
  const quickQueries = localizedList(hero, 'quickQueries');
  const groups = visibleGroupsForKind();
  if (state.group !== 'all' && !groups.some(({ id }) => id === state.group)) {
    state.group = 'all';
  }
  const capabilities = filteredCapabilities();
  const groupCounts = Object.fromEntries(
    groups.map((group) => [
      group.id,
      state.catalog.capabilities.filter(
        (capability) => capability.directoryGroupId === group.id
          && (state.kind === 'all' || capability.kind === state.kind),
      ).length,
    ]),
  );

  document.title = text(
    'OpenBitFun 使用指南 · Playbook — 功能目录、上手步骤与设置参考',
    'OpenBitFun Playbook — Practical feature guides & settings reference',
  );

  root.innerHTML = pageFrame(`
      <main>
        <section class="hero">
          <p class="eyebrow">${escapeHtml(localized(hero, 'kicker'))}</p>
          <h1><span>${escapeHtml(localized(hero, 'title'))}</span><em>${text(
            '既能亲手掌控，也能一句话交给智能体。',
            'Clear steps for you—or one sentence for your agent.',
          )}</em></h1>
          <p class="hero-copy">${escapeHtml(localized(hero, 'subtitle'))}</p>
          <div class="hero-actions">
            <form class="search-box" id="search-form" role="search">
              <span class="search-glyph" aria-hidden="true">⌕</span>
              <input id="capability-search" type="search" value="${escapeHtml(state.query)}"
                placeholder="${text('搜索你想做的事，例如：连接模型、远程 SSH、手机遥控、MCP…', 'Search what you want to do: connect model, remote SSH, MCP, dark mode…')}"
                autocomplete="off" spellcheck="false" />
              <button type="button" class="clear-search" id="clear-search" ${state.query ? '' : 'hidden'}
                aria-label="${text('清空搜索', 'Clear search')}">×</button>
              <kbd>⌘ K</kbd>
            </form>
            <div class="hero-stat" aria-label="${text('指南收录统计', 'Playbook coverage')}">
              <div class="stat-item"><strong>${state.catalog.counts.features}</strong><span>${text('项功能指南', 'feature guides')}</span></div>
              <i></i>
              <div class="stat-item"><strong>${state.catalog.counts.settings}</strong><span>${text('个设置页面', 'settings pages')}</span></div>
              <i></i>
              <div class="stat-item"><strong>${state.catalog.counts.documentedItems}</strong><span>${text('项细项说明', 'documented capabilities')}</span></div>
            </div>
          </div>
          <div class="quick-queries" aria-label="${text('热门搜索词', 'Suggested searches')}">
            <span class="quick-queries-label">${text('热门直达：', 'Try searching:')}</span>
            ${quickQueries.map((chip) => `
              <button type="button" class="quick-chip ${normalize(chip) === normalize(state.query) ? 'active' : ''}"
                data-quick-query="${escapeHtml(chip)}">${escapeHtml(chip)}</button>`).join('')}
          </div>
        </section>

        ${onboardingSectionsMarkup()}

        <section class="catalog-section" id="directory" aria-labelledby="catalog-heading">
          <div class="catalog-toolbar">
            <div>
              <p class="section-label">${text('功能与设置目录', 'Complete Directory')}</p>
              <h2 id="catalog-heading">${text('按任务领域浏览全部功能与设置', 'Browse features & settings by what you want to get done')}</h2>
            </div>
            <p id="result-count" aria-live="polite">${
              normalize(state.query)
                ? text(`找到 ${capabilities.length} 篇相关指南`, `${capabilities.length} matching guides`)
                : text(`共收录 ${capabilities.length} 篇指南`, `Showing ${capabilities.length} guides`)
            }</p>
          </div>

          <div class="filter-controls">
            <div class="kind-switch" role="tablist" aria-label="${text('目录类型', 'Guide type')}">
              <button class="${state.kind === 'all' ? 'active' : ''}" data-kind="all" type="button">
                ${text('全部目录', 'All guides')} <span>${state.catalog.counts.userFacing}</span>
              </button>
              <button class="${state.kind === 'feature' ? 'active' : ''}" data-kind="feature" type="button">
                ${text('功能指南', 'Features')} <span>${state.catalog.counts.features}</span>
              </button>
              <button class="${state.kind === 'setting' ? 'active' : ''}" data-kind="setting" type="button">
                ${text('设置参考', 'Settings')} <span>${state.catalog.counts.settings}</span>
              </button>
            </div>

            <div class="category-rail" role="tablist" aria-label="${text('功能领域分组', 'Functional directory groups')}">
              <button class="category-pill ${state.group === 'all' ? 'active' : ''}" data-group="all" type="button">
                ${text('全部分组', 'All groups')}
              </button>
              ${groups.map((group) => `
                <button class="category-pill ${state.group === group.id ? 'active' : ''}" data-group="${escapeHtml(group.id)}" type="button">
                  ${escapeHtml(groupTitle(group))} <span>${groupCounts[group.id] ?? 0}</span>
                </button>`).join('')}
            </div>
          </div>

          <div class="directory-results" id="capability-grid">
            ${directoryResultsMarkup(capabilities)}
          </div>
        </section>
      </main>
      ${footer()}
  `, 'index-page');

  bindCommonEvents();
  bindResetFilterButtons();

  document.querySelector('#search-form')?.addEventListener('submit', (event) => event.preventDefault());
  const searchInput = document.querySelector('#capability-search');
  let composing = false;
  const applySearchInput = () => {
    state.query = searchInput.value;
    syncIndexUrl();
    updateIndexResults();
  };
  searchInput?.addEventListener('compositionstart', () => {
    composing = true;
  });
  searchInput?.addEventListener('compositionend', () => {
    composing = false;
    applySearchInput();
  });
  searchInput?.addEventListener('input', (event) => {
    if (!composing && !event.isComposing) applySearchInput();
  });

  document.querySelector('#clear-search')?.addEventListener('click', () => {
    if (!searchInput) return;
    searchInput.value = '';
    state.query = '';
    syncIndexUrl(true);
    updateIndexResults();
    searchInput.focus();
  });

  document.querySelectorAll('[data-quick-query]').forEach((button) => {
    button.addEventListener('click', () => {
      const targetQuery = button.dataset.quickQuery ?? '';
      const nextQuery = normalize(state.query) === normalize(targetQuery) ? '' : targetQuery;
      state.query = nextQuery;
      if (searchInput) searchInput.value = nextQuery;
      syncIndexUrl(true);
      updateIndexResults();
    });
  });

  document.querySelectorAll('[data-kind]').forEach((button) => {
    button.addEventListener('click', () => {
      state.kind = button.dataset.kind;
      state.group = 'all';
      syncIndexUrl(true);
      renderIndex();
    });
  });

  document.querySelectorAll('[data-group]').forEach((button) => {
    button.addEventListener('click', () => {
      state.group = button.dataset.group;
      syncIndexUrl(true);
      renderIndex();
    });
  });
}

function schemaLabel(schema) {
  if (!schema) return '';
  if (schema.enum) {
    if (schema.enum.length <= 6) return schema.enum.map(String).join(' / ');
    return `${schema.enum.slice(0, 5).map(String).join(' / ')} ${text(`等 ${schema.enum.length} 项`, `+${schema.enum.length - 5} more`)}`;
  }
  if (schema.type === 'boolean') return text('开启 / 关闭', 'On / Off');
  if (schema.minimum !== undefined || schema.maximum !== undefined) {
    return `${schema.minimum ?? '…'} – ${schema.maximum ?? '…'}`;
  }
  if (schema.type === 'integer' || schema.type === 'number') return text('数值', 'Number');
  if (schema.type === 'string') return text('文本输入', 'Text');
  if (schema.type === 'array') return text('列表', 'List');
  if (schema.type === 'object') return text('结构化配置', 'Object');
  return schema.type;
}

function itemControlLabel(item) {
  if (item.control.kind === 'direct') return text('AI 可直接执行', 'Direct Agent control');
  if (item.control.kind === 'delegate') return text('AI 配合工具完成', 'Agent tool workflow');
  if (item.control.kind === 'open') return text('在界面中操作确认', 'Confirm in UI');
  return text('暂不支持', 'Unsupported');
}

function directActionsMarkup(capability) {
  if (!capability.operations?.length) return '';
  return `
    <div class="direct-operations-box">
      <h3>${text('智能体可直接帮你触发的操作', 'Actions your agent can trigger directly')}</h3>
      <div class="operation-list">
        ${capability.operations.map((operation) => `
          <article>
            <h4>${escapeHtml(localized(operation, 'title'))}</h4>
            <p>${escapeHtml(localized(operation, 'description'))}</p>
          </article>`).join('')}
      </div>
    </div>`;
}

function optionRowsMarkup(capability) {
  if (!capability.options?.length) {
    return `
      <div class="manual-empty">
        <p>${text(
          '这个设置页包含账号登录、连接测试或可视化表单等交互控件，需要在 OpenBitFun 界面中点选确认。你依然可以直接对智能体说一句话，让它先帮你打开到这个页面。',
          'This page uses interactive forms, connection tests, or credential inputs in the OpenBitFun interface. You can still ask your agent to open this exact page for you.',
        )}</p>
      </div>`;
  }
  return `
    <div class="option-table">
      ${capability.options.map((option) => `
        <article>
          <div class="option-meta">
            <h3>${escapeHtml(localized(option, 'title'))}</h3>
            <span class="option-schema-pill">${escapeHtml(schemaLabel(option.valueSchema))}</span>
          </div>
          <p>${escapeHtml(localized(option, 'description'))}</p>
        </article>`).join('')}
    </div>`;
}

function renderCapability(capability) {
  const group = groupForCapability(capability);
  const highlights = localizedList(capability, 'highlights');
  const scenarios = localizedList(capability, 'scenarios');
  const steps = localizedList(capability, 'steps');
  const examples = localizedList(capability, 'agentExamples');
  const tips = localizedList(capability, 'tips');
  const related = resolveRelatedCapabilities(state.catalog, capability, 4);
  const { previous, next } = resolveAdjacentCapabilities(state.catalog, capability.id);
  const showOptions = capability.kind === 'setting' || (capability.options?.length ?? 0) > 0;
  const hasAgentActions = Boolean(capability.operations?.length || capability.agentControl);
  const openPrompt = examples[0] || text(`帮我打开「${capability.titleZh}」`, `Open ${capability.titleEn}`);

  const tocSections = [
    (highlights.length || scenarios.length)
      ? { id: 'highlights', label: text('核心亮点与场景', 'Highlights & scenarios') }
      : null,
    { id: 'how-to', label: text('上手步骤', 'How to use it') },
    { id: 'ask-agent', label: text('直接对智能体说', 'Ask your agent') },
    { id: 'included', label: text(`包含的能力 (${capability.items.length})`, `What you can do (${capability.items.length})`) },
    showOptions ? { id: 'options', label: text('可调配置项', 'Configurable options') } : null,
    tips.length ? { id: 'tips', label: text('实用贴士', 'Tips & good to know') } : null,
  ].filter(Boolean);

  document.title = `${localized(capability, 'title')} — OpenBitFun Playbook`;

  root.innerHTML = pageFrame(`
      <main>
        <nav class="breadcrumb" aria-label="${text('面包屑导航', 'Breadcrumb')}">
          <a href="/">${text('指南首页', 'Home')}</a>
          <span aria-hidden="true">/</span>
          <a href="/?kind=${encodeURIComponent(capability.kind)}">${kindSectionHeading(capability.kind)}</a>
          <span aria-hidden="true">/</span>
          <a href="/?kind=${encodeURIComponent(capability.kind)}&group=${encodeURIComponent(capability.directoryGroupId)}">${escapeHtml(groupTitle(group))}</a>
        </nav>

        <article class="detail-layout manual-layout">
          <div class="detail-main">
            <header class="detail-header">
              <div class="detail-category kind-${escapeHtml(capability.kind)}">
                <span class="kind-badge kind-${escapeHtml(capability.kind)}">${kindLabel(capability.kind)}</span>
                <span>${escapeHtml(groupTitle(group))}</span>
              </div>
              <h1>${escapeHtml(localized(capability, 'title'))}</h1>
              <p class="detail-summary">${escapeHtml(localized(capability, 'summary'))}</p>

              <div class="entry-callout">
                <div class="entry-callout-info">
                  <span class="entry-callout-label">${text('在哪里找到它', 'Where to find it')}</span>
                  <div class="entry-callout-path">
                    <strong>${escapeHtml(localized(capability, 'entryPath'))}</strong>
                    ${capability.shortcut && !localized(capability, 'entryPath').includes(capability.shortcut)
                      ? `<kbd>${escapeHtml(capability.shortcut)}</kbd>`
                      : ''}
                  </div>
                </div>
                <button type="button" class="entry-copy-btn" data-copy-prompt="${escapeHtml(openPrompt)}">
                  <span>${text('复制对话指令', 'Copy prompt')}</span>
                </button>
              </div>
            </header>

            ${(highlights.length || scenarios.length) ? `
              <section class="content-section" id="highlights">
                <p class="section-label">${text('为什么值得用', 'Why It Helps')}</p>
                <h2>${text('核心亮点与适用场景', 'Key highlights & when to use it')}</h2>
                ${highlights.length ? `
                  <ul class="overview-highlights">
                    ${highlights.map((point) => `<li><span aria-hidden="true">✦</span><p>${escapeHtml(point)}</p></li>`).join('')}
                  </ul>` : ''}
                ${scenarios.length ? `
                  <div class="scenarios-callout">
                    <h3>${text('典型使用场景', 'Practical scenarios')}</h3>
                    <ul>
                      ${scenarios.map((scene) => `<li>${escapeHtml(scene)}</li>`).join('')}
                    </ul>
                  </div>` : ''}
              </section>` : ''}

            <section class="content-section" id="how-to">
              <p class="section-label">${text('一步步照着做', 'Step-by-Step')}</p>
              <h2>${text('怎么上手使用', 'How to use it')}</h2>
              <ol class="step-list">
                ${steps.map((step, index) => `
                  <li>
                    <span class="step-number">${index + 1}</span>
                    <p>${escapeHtml(step)}</p>
                  </li>`).join('')}
              </ol>
            </section>

            <section class="content-section" id="ask-agent">
              <p class="section-label">${text('交给智能体代劳', 'Collaborate with AI')}</p>
              <h2>${text('不想翻菜单？直接这样对智能体说', 'Prefer asking in chat? Try saying this')}</h2>
              <p class="section-intro">${
                hasAgentActions
                  ? text(
                    '点击任意一句话即可复制，粘贴到 OpenBitFun 会话框里——智能体会自动定位这项能力并帮你执行或打开。',
                    'Click any prompt to copy it into an OpenBitFun session—your agent will locate this capability and run or open it for you.',
                  )
                  : text(
                    '记不住设置路径也没关系。复制下面任意一句话发给智能体，它会直接帮你打开对应界面或引导你完成后续操作。',
                    'No need to memorize menus. Copy any sentence below into chat and your agent will open the right screen or guide you through.',
                  )
              }</p>
              <div class="prompt-list">
                ${examples.map((example) => `
                  <button type="button" data-copy-prompt="${escapeHtml(example)}">
                    <span>“${escapeHtml(example)}”</span>
                    <small>${text('复制', 'Copy')}</small>
                  </button>`).join('')}
              </div>
              ${directActionsMarkup(capability)}
            </section>

            <section class="content-section" id="included">
              <p class="section-label">${text(`功能细项一览 · 共 ${capability.items.length} 项`, `Capabilities Included · ${capability.items.length}`)}</p>
              <h2>${text('在这里你能做的全部事情', 'Everything you can do here')}</h2>
              <div class="control-legend" aria-label="${text('协作方式说明', 'Collaboration legend')}">
                <span><i class="legend-dot dot-direct"></i>${text('AI 可直接帮你改或执行', 'Agent can execute directly')}</span>
                <span><i class="legend-dot dot-delegate"></i>${text('AI 调用内置工具配合完成', 'Agent completes with built-in tools')}</span>
                <span><i class="legend-dot dot-open"></i>${text('AI 帮你打开入口，在界面中确认', 'Agent opens the screen; you confirm in UI')}</span>
              </div>
              <ul class="highlight-list inventory-list">
                ${capability.items.map((item) => {
                  const itemDesc = localized(item, 'description');
                  return `
                    <li id="item-${escapeHtml(item.id)}" data-inventory-item="${escapeHtml(item.id)}" data-control-kind="${escapeHtml(item.control.kind)}">
                      <div class="inventory-item-body">
                        <div class="inventory-item-head">
                          <em class="control-badge control-${escapeHtml(item.control.kind)}">${itemControlLabel(item)}</em>
                        </div>
                        <strong class="inventory-item-title">${escapeHtml(localized(item, 'title'))}</strong>
                        ${itemDesc ? `<p class="inventory-item-desc">${escapeHtml(itemDesc)}</p>` : ''}
                      </div>
                    </li>`;
                }).join('')}
              </ul>
            </section>

            ${showOptions ? `
              <section class="content-section" id="options">
                <p class="section-label">${text('配置参考', 'Settings Reference')}</p>
                <h2>${text('可调配置项说明', 'Configurable options')}</h2>
                ${optionRowsMarkup(capability)}
              </section>` : ''}

            ${tips.length ? `
              <section class="content-section" id="tips">
                <p class="section-label">${text('经验与避坑', 'Practical Tips')}</p>
                <h2>${text('实用贴士与注意事项', 'Tips & good to know')}</h2>
                <ul class="tips-list">
                  ${tips.map((tip) => `<li><span aria-hidden="true">💡</span><p>${escapeHtml(tip)}</p></li>`).join('')}
                </ul>
              </section>` : ''}

            <nav class="detail-pager" aria-label="${text('上下篇指南导航', 'Previous and next guides')}">
              ${previous ? `
                <a class="pager-card prev" href="${capabilityHref(previous)}">
                  <span class="pager-label">← ${text('上一篇', 'Previous')}</span>
                  <strong>${escapeHtml(localized(previous, 'title'))}</strong>
                </a>` : '<div></div>'}
              ${next ? `
                <a class="pager-card next" href="${capabilityHref(next)}">
                  <span class="pager-label">${text('下一篇', 'Next')} →</span>
                  <strong>${escapeHtml(localized(next, 'title'))}</strong>
                </a>` : '<div></div>'}
            </nav>
          </div>

          <aside class="detail-aside manual-aside">
            <div class="aside-card toc-card">
              <p class="aside-heading">${text('本页导览', 'On this page')}</p>
              <nav class="toc-links" aria-label="${text('本页章节导航', 'Page sections')}">
                ${tocSections.map((section) => `
                  <a href="#${escapeHtml(section.id)}">${escapeHtml(section.label)}</a>
                `).join('')}
              </nav>
            </div>

            <div class="aside-card agent-card">
              <p class="aside-heading">${text('一句话唤醒', 'Try in chat')}</p>
              <p class="agent-card-quote">“${escapeHtml(openPrompt)}”</p>
              <button type="button" class="copy-button" data-copy-first>${text('复制示例指令', 'Copy example prompt')}</button>
            </div>

            ${related.length ? `
              <div class="aside-card related-card">
                <p class="aside-heading">${text('相关功能与设置', 'Related guides')}</p>
                <div class="related-links">
                  ${related.map((entry) => `
                    <a class="related-link" href="${capabilityHref(entry)}">
                      <span class="kind-badge kind-${escapeHtml(entry.kind)}">${kindLabel(entry.kind)}</span>
                      <strong>${escapeHtml(localized(entry, 'title'))}</strong>
                    </a>`).join('')}
                </div>
              </div>` : ''}
          </aside>
        </article>
      </main>
      ${footer()}
  `, 'detail-page');

  bindCommonEvents();

  const focusedItemId = new URLSearchParams(location.search).get('focus');
  const focusedItem = focusedItemId
    ? document.querySelector(`[data-inventory-item="${CSS.escape(focusedItemId)}"]`)
    : null;
  if (focusedItem) {
    focusedItem.classList.add('focused');
    requestAnimationFrame(() => focusedItem.scrollIntoView({ block: 'center', behavior: 'smooth' }));
  }

  const copyPrompt = async (button, prompt) => {
    try {
      await navigator.clipboard.writeText(prompt);
    } catch {
      return;
    }
    const label = button.querySelector('small') ?? button.querySelector('span');
    const original = label ? label.textContent : button.textContent;
    if (label) label.textContent = text('已复制 ✓', 'Copied ✓');
    else button.textContent = text('已复制 ✓', 'Copied ✓');
    button.classList.add('copied');
    setTimeout(() => {
      button.classList.remove('copied');
      if (label) label.textContent = original;
      else button.textContent = original;
    }, 1800);
  };

  document.querySelectorAll('[data-copy-prompt]').forEach((button) => {
    button.addEventListener('click', () => void copyPrompt(button, button.dataset.copyPrompt));
  });
  document.querySelector('[data-copy-first]')?.addEventListener('click', (event) => {
    void copyPrompt(event.currentTarget, openPrompt);
  });
}

function setQuickJumpOpen(open) {
  state.quickJumpOpen = open;
  const modal = document.querySelector('#quick-jump-modal');
  if (!modal) return;
  modal.hidden = !open;
  document.body.classList.toggle('modal-open', open);
  if (open) {
    state.quickJumpActiveIndex = 0;
    const input = document.querySelector('#quick-jump-input');
    if (input) {
      input.value = state.quickJumpQuery;
      requestAnimationFrame(() => {
        input.focus();
        input.select();
      });
    }
    refreshQuickJumpList();
  } else {
    document.querySelector('#quick-jump-trigger')?.focus();
  }
}

function refreshQuickJumpList() {
  const list = document.querySelector('#quick-jump-list');
  if (!list) return;
  list.innerHTML = quickJumpResultsMarkup();
  list.querySelector('.quick-jump-item.active')?.scrollIntoView({ block: 'nearest' });
}

function bindCommonEvents() {
  const sidebarNav = document.querySelector('[data-sidebar-scroll]');
  const saveSidebarScroll = () => {
    if (sidebarNav) writeSessionPreference(SIDEBAR_SCROLL_STORAGE_KEY, String(sidebarNav.scrollTop));
  };
  sidebarNav?.addEventListener('scroll', saveSidebarScroll, { passive: true });
  document.querySelectorAll('.sidebar-home, [data-sidebar-item]').forEach((link) => {
    link.addEventListener('click', saveSidebarScroll);
  });
  const storedSidebarScroll = readSessionPreference(SIDEBAR_SCROLL_STORAGE_KEY);
  requestAnimationFrame(() => {
    if (!sidebarNav) return;
    const savedScrollTop = Number(storedSidebarScroll);
    if (storedSidebarScroll !== null && Number.isFinite(savedScrollTop) && savedScrollTop >= 0) {
      sidebarNav.scrollTop = savedScrollTop;
    }
    const activeItem = sidebarNav.querySelector('[aria-current="page"]');
    if (!activeItem) return;
    const navRect = sidebarNav.getBoundingClientRect();
    const itemRect = activeItem.getBoundingClientRect();
    if (itemRect.top < navRect.top || itemRect.bottom > navRect.bottom) {
      sidebarNav.scrollTop += itemRect.top - navRect.top - (navRect.height - itemRect.height) / 2;
    }
  });

  const setSidebarOpen = (open) => {
    document.body.classList.toggle('sidebar-open', open);
    document.querySelector('#sidebar-toggle')?.setAttribute('aria-expanded', String(open));
    if (open) document.querySelector('#sidebar-search')?.focus();
  };
  document.querySelector('#sidebar-toggle')?.addEventListener('click', () => setSidebarOpen(true));
  document.querySelector('#sidebar-close')?.addEventListener('click', () => setSidebarOpen(false));
  document.querySelector('#sidebar-backdrop')?.addEventListener('click', () => setSidebarOpen(false));

  const sidebarInput = document.querySelector('#sidebar-search');
  let sidebarComposing = false;
  const applySidebarFilter = () => {
    const needle = normalize(sidebarInput?.value ?? '');
    const tokens = needle.split(/\s+/u).filter(Boolean);
    document.querySelectorAll('[data-sidebar-item]').forEach((item) => {
      item.hidden = tokens.length > 0 && !tokens.every((token) => item.dataset.sidebarSearch.includes(token));
    });
    document.querySelectorAll('[data-sidebar-group]').forEach((group) => {
      group.hidden = !group.querySelector('[data-sidebar-item]:not([hidden])');
    });
    document.querySelectorAll('[data-sidebar-section]').forEach((section) => {
      section.hidden = !section.querySelector('[data-sidebar-item]:not([hidden])');
    });
    const hasResults = Boolean(document.querySelector('[data-sidebar-item]:not([hidden])'));
    const empty = document.querySelector('#sidebar-empty');
    if (empty) empty.hidden = hasResults;
  };
  sidebarInput?.addEventListener('compositionstart', () => {
    sidebarComposing = true;
  });
  sidebarInput?.addEventListener('compositionend', () => {
    sidebarComposing = false;
    applySidebarFilter();
  });
  sidebarInput?.addEventListener('input', (event) => {
    if (!sidebarComposing && !event.isComposing) applySidebarFilter();
  });

  document.querySelector('#language-toggle')?.addEventListener('click', () => {
    saveSidebarScroll();
    document.body.classList.remove('sidebar-open');
    state.language = state.language === 'zh' ? 'en' : 'zh';
    writePreference(LANGUAGE_STORAGE_KEY, state.language);
    render();
  });

  document.querySelectorAll('[data-theme-choice]').forEach((button) => {
    button.addEventListener('click', () => applyTheme(button.dataset.themeChoice));
  });

  // Quick Jump modal bindings
  document.querySelector('#quick-jump-trigger')?.addEventListener('click', () => setQuickJumpOpen(true));
  document.querySelector('#quick-jump-close')?.addEventListener('click', () => setQuickJumpOpen(false));
  document.querySelector('#quick-jump-backdrop')?.addEventListener('click', () => setQuickJumpOpen(false));

  const quickInput = document.querySelector('#quick-jump-input');
  let quickComposing = false;
  const applyQuickInput = () => {
    state.quickJumpQuery = quickInput?.value ?? '';
    state.quickJumpActiveIndex = 0;
    refreshQuickJumpList();
  };
  quickInput?.addEventListener('compositionstart', () => {
    quickComposing = true;
  });
  quickInput?.addEventListener('compositionend', () => {
    quickComposing = false;
    applyQuickInput();
  });
  quickInput?.addEventListener('input', (event) => {
    if (!quickComposing && !event.isComposing) applyQuickInput();
  });
  quickInput?.addEventListener('keydown', (event) => {
    if (quickComposing || event.isComposing || event.keyCode === 229) return;
    const items = [...document.querySelectorAll('[data-quick-jump-index]')];
    if (!items.length) return;
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      state.quickJumpActiveIndex = (state.quickJumpActiveIndex + 1) % items.length;
      refreshQuickJumpList();
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      state.quickJumpActiveIndex = (state.quickJumpActiveIndex - 1 + items.length) % items.length;
      refreshQuickJumpList();
    } else if (event.key === 'Enter') {
      const target = items[state.quickJumpActiveIndex] ?? items[0];
      if (target?.getAttribute('href')) {
        event.preventDefault();
        location.href = target.getAttribute('href');
      }
    }
  });

  document.querySelectorAll('[data-quick-modal-chip]').forEach((chip) => {
    chip.addEventListener('click', () => {
      state.quickJumpQuery = chip.dataset.quickModalChip ?? '';
      state.quickJumpActiveIndex = 0;
      if (quickInput) {
        quickInput.value = state.quickJumpQuery;
        quickInput.focus();
      }
      refreshQuickJumpList();
    });
  });
}

function render() {
  document.documentElement.lang = state.language === 'zh' ? 'zh-CN' : 'en';
  const targetCapabilityId = activeCapabilityId();
  if (targetCapabilityId) {
    const capability = state.catalog.capabilities.find(({ id }) => id === targetCapabilityId);
    if (capability) renderCapability(capability);
    else renderIndex();
  } else {
    renderIndex();
  }
}

window.addEventListener('popstate', () => {
  if (!state.catalog) return;
  const params = new URLSearchParams(location.search);
  const nextKind = params.get('kind');
  state.query = params.get('q') ?? '';
  state.kind = ['all', 'feature', 'setting'].includes(nextKind) ? nextKind : 'all';
  state.group = params.get('group') ?? 'all';
  render();
});

document.addEventListener('keydown', (event) => {
  if (event.isComposing || event.keyCode === 229) return;
  if (event.key === 'Escape') {
    if (state.quickJumpOpen) {
      event.preventDefault();
      setQuickJumpOpen(false);
      return;
    }
    if (document.body.classList.contains('sidebar-open')) {
      document.body.classList.remove('sidebar-open');
      document.querySelector('#sidebar-toggle')?.setAttribute('aria-expanded', 'false');
      document.querySelector('#sidebar-toggle')?.focus();
      return;
    }
  }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
    event.preventDefault();
    setQuickJumpOpen(!state.quickJumpOpen);
    return;
  }
  if (
    event.key === '/'
    && !event.metaKey
    && !event.ctrlKey
    && !event.altKey
    && !state.quickJumpOpen
    && !activeCapabilityId()
    && !['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement?.tagName ?? '')
    && !document.activeElement?.isContentEditable
  ) {
    const indexSearch = document.querySelector('#capability-search');
    if (indexSearch) {
      event.preventDefault();
      indexSearch.focus();
      indexSearch.select();
    }
  }
});

window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
  if (state.theme === 'system') applyTheme('system', false);
});

applyTheme(state.theme, false);

try {
  const response = await fetch('/data/capabilities.json');
  if (!response.ok) throw new Error(`Catalog request failed: ${response.status}`);
  state.catalog = normalizePlaybookCatalog(await response.json());
  render();
} catch (error) {
  root.innerHTML = `<div class="fatal-error"><h1>OpenBitFun Playbook</h1><p>${escapeHtml(error.message)}</p></div>`;
}
