import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const testPackages = createRequire(new URL('../tests/e2e/package.json', import.meta.url));
const puppeteer = createRequire(testPackages.resolve('@wdio/cli'))('puppeteer-core');
const url = process.env.OPENBITFUN_VISUALIZE_TEST_URL;
if (!url) throw Error('Set OPENBITFUN_VISUALIZE_TEST_URL to the Vite /tests/e2e/visualize.html fixture.');
const browser = await puppeteer.launch({
  headless: true,
  ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : { channel: 'chrome' }),
});
try {
  const page = await browser.newPage();
  page.setDefaultNavigationTimeout(90000);
  const errors = [];
  const archiveModules = new Set();
  page.on('request', request => {
    const path = new URL(request.url()).pathname;
    if (/\/generative-ui\/(?:genUIOpenUI|GenUIApplication)\.tsx$|@openuidev[_/]react-lang/.test(path)) {
      archiveModules.add(path);
    }
  });
  page.on('pageerror', (error) => errors.push(String(error)));
  await page.setViewport({ width: 736, height: 800 });
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  const chart = 'figure.visual-answer:first-of-type';
  await page.waitForSelector('.visual-answer [role="img"] svg', { timeout: 90000 });
  await page.waitForSelector('.visual-answer input[type="range"]');
  assert.equal(await page.$$eval('figure.visual-answer', (nodes) => nodes.length), 2);
  assert.equal(await page.$$eval('[data-flow-item-type="tool"], .flow-tool-card-wrapper', nodes => nodes.length), 0);
  await page.select(chart + ' label > select', '2');
  const bar = await page.waitForSelector(chart + ' svg path[fill]:not([fill="none"]):last-of-type');
  await bar.click();
  await page.waitForFunction(() => document.querySelector('.visual-answer__controls select')?.value === '1');
  await page.evaluate(() => {
    window.addEventListener(
      'fill-chat-input',
      (e) => {
        document.body.dataset.followup = JSON.stringify(e.detail);
      },
      { once: true },
    );
  });
  await page.click(chart + ' button');
  const draft = JSON.parse(await page.$eval('body', (body) => body.dataset.followup));
  assert.equal(draft.sessionId, 'visual-test');
  assert.equal(draft.append, true);
  assert.match(draft.message, /Service = B/);
  assert.match(draft.message, /Latency = 103/);
  await page.focus('input[type="range"]');
  await page.keyboard.press('End');
  assert.equal(await page.$eval('.visual-answer dd', (el) => el.textContent), '60');
  await page.reload();
  await page.waitForSelector('.visual-answer svg');
  assert.equal(await page.$eval(chart + ' label > select', (el) => el.value), '2');
  assert.equal(await page.$eval('.visual-answer dd', (el) => el.textContent), '60');
  await page.setViewport({ width: 320, height: 640 });
  await page.waitForFunction(() => document.documentElement.scrollWidth <= window.innerWidth);
  await page.goto(url + '?catalog');
  await page.waitForFunction(
    () => document.querySelectorAll('.visual-answer [role="img"] svg').length === 15,
  );
  assert.equal(await page.$$eval('figure.visual-answer', (nodes) => nodes.length), 18);
  assert.equal(await page.$eval('.visual-answer__days', (el) => el.textContent.includes('Standup')), true);
  const calendar = 'figure.visual-answer:has(.visual-answer__days)';
  await page.select(calendar + ' select', 'month');
  assert.equal(await page.$$eval(calendar + ' section', (nodes) => nodes.length), 30);
  await page.click('.visual-answer__frames li:nth-child(2) button');
  assert.equal(
    await page.$eval('.visual-answer__frames button[aria-pressed="true"]', (el) =>
      el.textContent.includes('main'),
    ),
    true,
  );
  const chartKinds = [
    'line',
    'area',
    'bar',
    'pie',
    'histogram',
    'waterfall',
    'heatmap',
    'scatter',
    'funnel',
    'box-plot',
    'sparkline',
    'ranking',
    'sankey',
  ];
  for (const kind of chartKinds) {
    const handle = await page.evaluateHandle(
      (k) =>
        [...document.querySelectorAll('figure.visual-answer')].find(
          (f) => f.querySelector('figcaption')?.textContent === k,
        ),
      kind,
    );
    const region = handle.asElement();
    assert.ok(region, kind);
    const plot = await region.$('[role="img"]');
    await plot.focus();
    await page.keyboard.press('Home');
    const first = await region.$eval('output', (e) => e.textContent);
    assert.ok(first.length > 0, kind + ' keyboard inspection');
    await page.keyboard.press('End');
    assert.ok(await region.$eval('output', (e) => e.textContent.length > 0), kind);
    await page.keyboard.press('Escape');
    assert.equal(await region.$('output'), null, kind + ' Escape');
    await handle.dispose();
  }
  await page.goto(url + '?widgets&light');
  await page.waitForSelector('[role="tab"]');
  const figure = async (caption) =>
    (
      await page.evaluateHandle(
        (c) =>
          [...document.querySelectorAll('.visual-answer')].find(
            (f) => f.querySelector('figcaption')?.textContent === c,
          ),
        caption,
      )
    ).asElement();
  const clickNamed = async (root, name) => {
    const b = (
      await root.evaluateHandle(
        (el, n) => [...el.querySelectorAll('button')].find((b) => b.textContent.trim() === n),
        name,
      )
    ).asElement();
    assert.ok(b, name);
    await b.click();
  };
  const output = async (root) => root.$eval('output', (el) => el.textContent);
  let cal = await figure('calendar');
  await (await cal.$('select')).select('week');
  await clickNamed(cal, 'Standup · 09:00–09:30');
  assert.match(await output(cal), /Release team/);
  let water = await figure('request-waterfall');
  await clickNamed(water, 'Request');
  assert.match(await output(water), /Self time 40 ms/);
  assert.match(await output(water), /orders/);
  await (await water.$('[aria-expanded]')).click();
  assert.equal(await water.$$eval('.visual-answer__span', (els) => els.length), 1);
  await (await water.$('[aria-expanded]')).click();
  const checkboxes = await water.$$('[type="checkbox"]');
  await checkboxes[0].click();
  assert.equal(await water.$$eval('.visual-answer__span:first-child i', (els) => els.length), 2);
  await checkboxes[1].click();
  assert.equal(await water.$$eval('.visual-answer__span', (els) => els.length), 2);
  await (await water.$('[aria-label="Zoom in"]')).click();
  assert.equal(await water.$eval('[type="range"]', (el) => el.max), '60');
  let flame = await figure('flame-chart');
  const calls = await flame.$$('.visual-answer__level:first-child button');
  assert.equal(calls.length, 2);
  await calls[1].click();
  assert.match(await output(flame), /Start 4 ms.*Self time 2 ms/);
  await (await flame.$('summary')).click();
  assert.match(await flame.$eval('tbody', (el) => el.textContent), /render5 ms2 ms/);
  const trace = await figure('stack-trace');
  await clickNamed(trace, 'load · app.ts · 8');
  assert.match(await trace.$eval('pre [data-current=true]', (el) => el.textContent), /8.*fail/);
  await (await trace.$('[aria-expanded]')).click();
  assert.match(await trace.evaluate((el) => el.textContent), /dispatch/);
  assert.match(await trace.evaluate((el) => el.textContent), /raw traceback line/);
  let tabs = await figure('tabs');
  const drawn = await tabs.$eval('.visual-guide__node .visual-guide__accent', el => el.getAttribute('d'));
  assert.equal(drawn, 'M10 70Q73 5 136 70');
  assert.equal(await tabs.$eval('progress', (el) => el.value), 6);
  assert.match(await tabs.$eval('table', (el) => el.textContent), /OK6/);
  await (await tabs.$('[role="tab"]')).focus();
  await page.keyboard.press('ArrowRight');
  await (await tabs.$('select')).select('Worker');
  await (await tabs.$('input[type="checkbox"]')).click();
  await (await tabs.$('input:not([type])')).type('p95');
  await (await tabs.$('input[type="range"]')).focus();
  await page.keyboard.press('End');
  await page.evaluate(() =>
    window.addEventListener(
      'fill-chat-input',
      (e) => {
        document.body.dataset.followup = e.detail.message;
      },
      { once: true },
    ),
  );
  await clickNamed(tabs, 'Ask about selection');
  const formDraft = await page.$eval('body', (el) => el.dataset.followup);
  for (const field of ['Service = Worker', 'Include retries = true', 'Note = p95', 'Budget = 10'])
    assert.ok(formDraft.includes(field));
  const trend = await figure('Trend');
  const mounted = await trend.$eval('[role="img"]', (el) => el.getAttribute('_echarts_instance_'));
  await clickNamed(trend, 'B');
  await (await trend.$('[role="img"]')).focus();
  await page.keyboard.press('Home');
  assert.match(await output(trend), /A = 4 \(100%\)/);
  assert.equal(await trend.$eval('[role="img"]', (el) => el.getAttribute('_echarts_instance_')), mounted);
  await clickNamed(trend, 'B');
  const ranking = await figure('Ranking');
  await clickNamed(ranking, 'Show more');
  assert.equal(await ranking.$('button'), null);
  const comparison = 'section[aria-label="comparison"]';
  await page.select(comparison + ' > label select', '2');
  await (await ranking.$('[role="img"]')).focus();
  await page.keyboard.press('End');
  assert.match(await output(ranking), /Service 1(?:\s|$)/);
  await page.reload();
  await page.waitForSelector('[role="tab"]');
  assert.equal(await page.$eval(comparison + ' > label select', (e) => e.value), '2');
  cal = await figure('calendar');
  assert.equal(await cal.$eval('select', (e) => e.value), 'week');
  assert.match(await output(cal), /Release team/);
  water = await figure('request-waterfall');
  assert.equal(await water.$eval('[type="range"]', (el) => el.max), '60');
  flame = await figure('flame-chart');
  assert.match(await output(flame), /Start 4 ms/);
  tabs = await figure('tabs');
  assert.equal(await tabs.$eval('select', (e) => e.value), 'Worker');
  await page.waitForFunction(() => document.documentElement.scrollWidth <= window.innerWidth);
  await page.goto(url + '?html-controls');
  const frameElement = await page.waitForSelector('iframe[data-gen-ui-html-frame]');
  assert.equal(await frameElement.evaluate(el => el.getAttribute('sandbox')), '');
  assert.ok((await frameElement.boundingBox()).height > 260);
  const frame = await page.waitForFrame(async candidate =>
    candidate.parentFrame() === page.mainFrame() &&
    await candidate.evaluate(() => Boolean(document.querySelector('label[for="services"]')?.getClientRects().length)).catch(() => false),
  );
  assert.equal(await frame.$eval('#details', el => getComputedStyle(el).display), 'none');
  await frame.click('label[for="services"]');
  assert.equal(await frame.$eval('#details', el => getComputedStyle(el).display), 'block');
  assert.equal(await page.$('.flow-tool-card-wrapper'), null);

  await page.setViewport({ width: 810, height: 700 });
  await page.goto(url + '?architecture&light');
  const layerSelector = '.visual-layers__choice';
  await page.waitForSelector(layerSelector);
  assert.equal(await page.$$eval(layerSelector, nodes => nodes.length), 6);
  const checkLayer = async index => {
    assert.equal(await page.$eval(layerSelector + ':has(:checked)', el => el.textContent),
      await page.$eval('.visual-layers__detail h3', el => el.textContent));
    assert.equal(await page.$$eval(layerSelector + ' input', inputs => inputs.findIndex(input => input.checked)), index);
  };
  await checkLayer(0);
  const columns = () => page.$eval('.visual-layers__body', el => getComputedStyle(el).gridTemplateColumns.split(' ').length);
  assert.equal(await columns(), 2);
  for (const [i, control] of (await page.$$(layerSelector)).entries()) {
    await control.click();
    await checkLayer(i);
  }
  await page.focus(layerSelector + ' input:checked');
  await page.keyboard.press('ArrowUp');
  await checkLayer(4);
  await page.goto(url + '?architecture');
  await page.waitForSelector(layerSelector);
  await checkLayer(4);
  assert.equal(await page.$eval('.visual-layers__path', el => el.textContent), 'src/crates/execution');
  assert.ok(await page.$eval('.visual-layers__detail ul', el => el.textContent.includes('agent-runtime')));
  await page.setViewport({ width: 320, height: 640 });
  await page.waitForFunction(() => document.documentElement.scrollWidth <= window.innerWidth);
  assert.equal(await columns(), 1);
  assert.equal(await page.$('.flow-tool-card-wrapper, .visual-layers iframe'), null);
  console.log('Passed: six native layers, pointer and keyboard selection, reading state, responsive layout, both themes.');

  await page.setViewport({ width: 810, height: 720 });
  await page.goto(url + '?overview&light');
  await page.waitForSelector('.visual-guide__node svg path');
  const tabsInGuide = '.visual-guide [role=tab]';
  const choices = '.visual-guide__variants button';
  const contents = selector => page.$$eval(selector, nodes => nodes.map(node => node.textContent));
  assert.equal((await contents(tabsInGuide)).length, 3);
  assert.equal((await contents('.visual-guide__node h3')).length, 3);
  const steps = new Set();
  for (const choice of await page.$$(choices)) {
    await choice.click();
    assert.equal(await choice.evaluate(el => el.getAttribute('aria-pressed')), 'true');
    steps.add(await page.$eval('.visual-guide__steps', el => el.textContent));
  }
  assert.equal(steps.size, 4);
  await page.focus(tabsInGuide);
  await page.keyboard.press('ArrowRight');
  assert.equal((await contents('.visual-guide__outline li')).length, 6);
  await page.keyboard.press('ArrowRight');
  const routes = new Set();
  for (const choice of await page.$$(choices)) {
    await choice.click();
    assert.equal(await choice.evaluate(el => el.getAttribute('aria-pressed')), 'true');
    routes.add((await contents('.visual-guide__node h3')).join('/'));
  }
  assert.equal(routes.size, 4);
  const route = await contents('.visual-guide__node h3');
  await page.goto(url + '?overview');
  await page.waitForSelector('.visual-guide__node');
  assert.deepEqual(await contents('.visual-guide__node h3'), route);
  assert.equal(await page.$eval('.visual-guide__diagram', el => getComputedStyle(el).flexDirection), 'row');
  await page.setViewport({ width: 320, height: 640 });
  assert.equal(await page.$eval('.visual-guide__diagram', el => getComputedStyle(el).flexDirection), 'column');
  await page.waitForFunction(() => document.documentElement.scrollWidth <= window.innerWidth);
  assert.equal(await page.$('.flow-tool-card-wrapper, .visual-guide iframe'), null);
  console.log('Passed: reference overview, three tabs, four modes, six architecture rows, four distinct routes and retained choice.');

  assert.deepEqual([...archiveModules], [], 'New answers must load independently of the OpenUI renderer');
  await page.goto(url + '?history');
  await page.waitForFunction(() => document.querySelectorAll('[data-gen-ui-app]').length === 2);
  for (const i of [0, 1]) {
    const region = await page.$('[data-gen-ui-app="archive-' + i + '"]');
    assert.match(await region.evaluate((el) => el.textContent), new RegExp('Saved record ' + i));
    assert.equal(await region.$eval('button', (e) => e.disabled), true);
  }
  assert.ok(archiveModules.size, 'Old messages load the archive renderer on demand');
  assert.equal(await page.$('[data-openbitfun-state="repairRequired"]'), null);
  console.log('Passed: direct and deferred archived GenUI results render read-only.');
  assert.deepEqual(errors, []);
  console.log('Passed: 13 chart families, 4 specialist views, parameter simulator.');
  console.log(
    'Passed: two inline Write snapshots, deferred history, SVG chart, filter, follow-up draft, keyboard simulation, reload, narrow viewport.',
  );
} finally {
  await browser.close();
}
