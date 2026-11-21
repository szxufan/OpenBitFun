import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';

const packages = createRequire(new URL('../tests/e2e/package.json', import.meta.url));
const puppeteer = createRequire(packages.resolve('@wdio/cli'))('puppeteer-core');
const root = new URL(
  '../src/crates/assembly/core/builtin_skills/visualize/upstream/skills/visualize/',
  import.meta.url,
);
const url = new URL(
  process.env.OPENBITFUN_VISUALIZE_TEST_URL ?? 'http://localhost:1422/tests/e2e/visualize.html',
);
const narrow = process.argv.includes('--narrow-dark');
url.search = narrow ? 'codex' : 'codex&light';
const browser = await puppeteer.launch({
  headless: true,
  ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : { channel: 'chrome' }),
});
const page = await browser.newPage();
page.on('requestfailed', (req) => {
  if (req.url().startsWith('https://')) console.error(req.failure()?.errorText, req.url().split('?')[0]);
});
page.on('pageerror', (error) => console.error(error.message));
page.setDefaultTimeout(30000);
async function testHost() {
  await page.setViewport({ width: narrow ? 375 : 840, height: 850 });
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  async function open(html) {
    await page.evaluate((value) => sessionStorage.setItem('visualize-test-fragment', value), html);
    await page.reload({ waitUntil: 'domcontentloaded' });
    const iframe = await page.waitForSelector('[data-codex-figure][data-figure-check=passed] iframe[srcdoc]');
    assert.equal(await iframe.evaluate((e) => e.getAttribute('sandbox')), 'allow-scripts');
    assert.equal(await page.$('.flow-tool-card-wrapper'), null);
    const until = Date.now() + 30000;
    while (Date.now() < until) {
      const container = await iframe.contentFrame();
      const frame = container?.childFrames()[0] ?? container;
      if (
        frame &&
        (await frame
          .evaluate(() => typeof window.openai?.setWidgetState === 'function' && !!document.body)
          .then(Boolean)
          .catch(() => false))
      )
        return frame;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw Error('The visualization did not initialize in FlowChat');
  }
  async function switchAppearance() {
    await page.evaluate(async () => {
      const { appearanceRuntime, buildBuiltinAppearance, openBitFunLightPalette, openBitFunMidnightPalette } =
        await import('/src/infrastructure/appearance/index.ts');
      const dark = document.documentElement.dataset.openbitfunAppearanceMode !== 'dark';
      await appearanceRuntime.applyPackage(buildBuiltinAppearance(dark ? openBitFunMidnightPalette : openBitFunLightPalette));
      document.documentElement.dataset.colorScheme = dark ? 'dark' : 'light';
    });
  }
  // Sample what the browser actually paints inside each component, including its shadow tree.
  async function verifyRepaint(frame, selector, count) {
    const reader = await frame.evaluateHandle(() => selector => [...document.querySelectorAll(selector)].map(host => {
      const paint = new Set();
      function visit(node) {
        if (node.getClientRects().length && node.tagName !== 'STYLE') {
          const css = getComputedStyle(node);
          const attributes = node instanceof SVGElement ? ['fill', 'stroke'] : ['color', 'backgroundColor', 'borderTopColor'];
          for (const key of attributes) paint.add(css[key]);
        }
        for (const el of (node.shadowRoot ?? node).children) visit(el);
      }
      visit(host);
      return [...paint].sort().join('|');
    }));
    const before = await frame.evaluate((read, selector) => read(selector), reader, selector);
    assert.equal(before.length, count);
    await switchAppearance();
    await frame.waitForFunction((read, selector, old) => {
      const next = read(selector);
      return next.length === old.length && next.every((paint, i) => paint && paint !== old[i]);
    }, {}, reader, selector, before);
    await reader.dispose();
  }
  const photo = 'https://images.example.test/diagram.png';
  const dependency = 'https://cdn.jsdelivr.net/npm/figure-replay-fixture@1/index.js';
  let scriptLoads = 0;
  await page.setRequestInterception(true);
  page.on('request', request => {
    if (request.url() === dependency) {
      scriptLoads++;
      if (scriptLoads > 1) return void request.abort();
      return void request.respond({
        status: 200, contentType: 'application/javascript', headers: { 'Access-Control-Allow-Origin': '*' },
        body: 'document.querySelector("output").textContent = window.openai.widgetState?.privateContent?.choice ?? "first";',
      });
    }
    if (request.url() === photo) {
      void request.respond({
        status: 200, contentType: 'image/png', headers: { 'Access-Control-Allow-Origin': '*' },
        body: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aT1sAAAAASUVORK5CYII=', 'base64'),
      });
    } else if (process.env.OPENBITFUN_VISUALIZE_OFFLINE === '1' && request.url().startsWith('https://')) {
      void request.abort();
    } else void request.continue();
  });
  const privateFigure = await open('<!-- vis-bindings\n' + JSON.stringify({
    rows: { type: 'json', value: [2, 3] }, photo: { type: 'image', url: photo },
  }) + '\n-->\n' +
    '<output>Loading</output><img alt="Diagram"><script>const visual = window.openai.visualization;' +
    'visual.onData("rows", rows => { document.querySelector("output").textContent=String(rows.length); });' +
    'visual.onData("photo", src => { document.querySelector("img").src=src; });</script>');
  assert.equal(await privateFigure.$eval('output', el => el.textContent), '2');
  await privateFigure.waitForFunction(() => document.querySelector('img').naturalWidth === 1);
  assert.match(await privateFigure.$eval('img', el => el.src), /^data:image\/png;base64,/);
  for (const frame of [privateFigure, privateFigure.parentFrame()]) {
    const policy = await frame.$eval('meta[http-equiv="Content-Security-Policy"]', el => el.content);
    assert.ok(policy.includes("frame-src 'none'"));
    assert.ok(!policy.includes('https:'));
  }
  const checkedCsv = await page.evaluate(async () => {
    const { inspectWrittenFigure } = await import('/src/tools/visualize/backgroundInspection.ts');
    const root = 'openbitfun://current-session/artifacts/visualizations/v2/';
    const csv = 'city,count\nShanghai,3';
    const request = { sessionId: 'owner', turnId: 'turn', toolCallId: 'plot', uri: root + 'map.html',
      source: '<!-- vis-bindings\n{"rows":{"type":"file","path":"./cities.csv"}}\n-->\n' +
        '<main data-visual-map data-map-source="https://example.test/regions"><svg viewBox="0 0 120 80"></svg></main><script>openai.visualization.onData("rows", async f => {' +
        `if ((await f.blob.text()).includes("Shanghai,3")) document.querySelector("svg").innerHTML='<path data-visual-region d="M2 3L90 4L60 60Z"/>'; });</script>`,
    };
    const file = { type:'tool', toolName:'Write', status:'completed', toolCall: { id:'csv', input:{payload:'+++ '+root+'cities.csv\n'+csv} },
      toolResult:{success:true,result:{file_path:root+'cities.csv',content_bytes:csv.length}} };
    const write = { type:'tool', toolName:'Write', toolCall:{id:'plot',input:{}} };
    const session = { sessionId:'owner', dialogTurns:[{ id:'turn',modelRounds:[{items:[file,write]}] }] };
    const placeholder = { ...request, source: request.source.replace(/<main[\s\S]*/, '<h2>City counts</h2><output>Loading CSV</output>') };
    file.status = 'running';
    const pending = await inspectWrittenFigure(placeholder, async () => session);
    file.status = 'completed';
    return { pending, checked: await inspectWrittenFigure(request, async () => session),
      unattached: await inspectWrittenFigure(request, async () => undefined) };
  });
  assert.deepEqual(checkedCsv.pending, { deferred: true });
  assert.equal(checkedCsv.checked.bindings.rows.value.text, 'city,count\nShanghai,3');
  assert.deepEqual(checkedCsv.unattached, { deferred: true });
  console.log('Passed private frame policy and background rendering with the producing call’s CSV snapshot.');
  const lateHtml = '<!-- vis-bindings\n{"rows":{"type":"file","path":"./cities.csv"}}\n-->\n' +
    '<h2>City counts</h2><output id="cities">Loading CSV</output><script>' +
    'openai.visualization.onData("rows",async f=>{if(f) document.querySelector("#cities").textContent=await f.blob.text()});</script>';
  const waiting = await open(lateHtml);
  assert.equal(await waiting.$eval('#cities', el => el.textContent), 'Loading CSV');
  for (const status of ['running', 'completed']) {
    await page.evaluate(status => window.dispatchEvent(new CustomEvent('fixture:csv', { detail: status })), status);
  }
  await page.waitForFrame(async frame => frame.waitForFunction(
    () => document.querySelector('#cities')?.textContent === 'Shanghai,3', { timeout: 2000 },
  ).then(() => true, () => false), { timeout: 15000 });
  console.log('Passed recovery of a mounted figure with an old empty snapshot after its CSV completed.');
  const background = await page.evaluate(async () => {
    const { inspectInBackground } = await import('/src/tools/visualize/backgroundInspection.ts');
    const backgroundFrames = () => document.querySelectorAll('body > iframe[inert][aria-hidden="true"]').length;
    const before = backgroundFrames();
    const checks = await Promise.all([
      inspectInBackground('<main data-visual-map><svg><path d="M5 5L50 45L80 20Z"/></svg></main>'),
      inspectInBackground('<main data-visual-map><svg></svg></main>'),
      inspectInBackground('<div data-visual-motion style="width:20px;height:20px">stationary</div>'),
      inspectInBackground('<output>broken</output><script>throw Error("fixture-missing-input")</script>'),
    ]);
    return { checks, extraFrames: backgroundFrames() - before };
  });
  assert.equal(background.checks[0], null);
  assert.match(background.checks[1], /no visible projected geometry/);
  assert.match(background.checks[2], /stationary/);
  assert.match(background.checks[3], /fixture-missing-input|Script error/);
  assert.equal(background.extraFrames, 0);
  const selection = await page.evaluate(async () => {
    const { inspectInBackground } = await import('/src/tools/visualize/backgroundInspection.ts');
    const fragment = '<select><option value="0">19:30</option><option value="30">20:00</option></select><output>19:30</output>' +
      '<script>const picker = document.querySelector("select"); picker.onchange = () => document.querySelector("output").textContent = ["19:30","20:00"][+picker.value].trim()</script>';
    return Promise.all([
      inspectInBackground(fragment, {}, { mapExpected: false }),
      inspectInBackground(fragment.replace('+picker.value', 'picker.selectedIndex'), {}, { mapExpected: false }),
      inspectInBackground(fragment),
    ]);
  });
  assert.match(selection[0], /Selecting "20:00" failed/);
  assert.deepEqual(selection.slice(1), [null, null]);
  console.log('Passed background checks, actionable errors and frame cleanup.');
  const mapPublication = await page.evaluate(async () => {
    const { inspectWrittenFigure } = await import('/src/tools/visualize/backgroundInspection.ts');
    const { materializeRegionalMaps } = await import('/src/tools/visualize/regionalAtlas.ts');
    const route = '<svg aria-label="Walking route map" viewBox="0 0 300 100"><path d="M5 10L220 45"/><text y="80">1 km</text></svg>';
    const write = async (name, source, prompt = '') => {
      try { await inspectWrittenFigure({ sessionId: 'session', turnId: 'turn', uri: 'openbitfun://current-session/artifacts/visualizations/v2/' + name + '.html', source, userRequest: prompt }, async () => { throw Error('Unexpected history lookup'); }); }
      catch (error) { return error.message; }
      return null;
    };
    return Promise.all([
      write('outings', route),
      write('map-trip', '<svg><path d="M5 10L220 45"/></svg>'),
      write('map-trip', '<p>Street data is unavailable.</p><ol data-visual-itinerary><li>First stop</li></ol>'),
      write('clock', '<svg aria-label="Pendulum"><path d="M5 10L50 45"/></svg>'),
      materializeRegionalMaps('<script type="application/json" data-visual-atlas="1">{"detail":"street","places":[]}</script>')
        .then(() => null, error => error.message),
      write('outing', '<h2>Hangzhou</h2><p>West Lake</p>', '画一张杭州地图，标出西湖。'),
      write('concept', '<h2>Architecture</h2>', 'Show me a map of the codebase.'),
      write('hangzhou-route-map', '<ol><li>First stop</li></ol>', '地图给我看看'),
      write('map-itinerary', '<h2>杭州一日出行规划</h2><script type="application/json" data-visual-atlas="1">{"places":["杭州"]}</script>'),
      write('map-private', '<!-- vis-bindings\n{"rows":{"type":"json","value":[]}}\n-->\n<script type="application/json" data-visual-atlas="1">{"detail":"street","city":"Paris","places":["Private stop"]}</script>'),
      ...['地图呢？', 'Where’s the map?'].map(prompt =>
        write('evening', '<ol data-visual-itinerary><li>First stop</li></ol>', prompt)),
      write('schedule', '<viz-calendar></viz-calendar><h2>行程表</h2>', '画一张杭州地图，并给出行程表。'),
      write('map-hangzhou', '<h2>杭州</h2>', '画一张杭州地图，并给出行程表。'),
      write('map-services', '<h2>Services</h2>', 'Show me a map of our microservices.'),
      write('journey', '<h2>User journey</h2>', 'Show me a map of the user journey.'),
      write('map-plan', '<ol data-visual-itinerary><li><svg aria-hidden="true" width="16" height="16"><path d="M0 0L9 9"/></svg>First stop</li></ol>'),
      write('outing', '<h2>Paris</h2>', 'Show me a map of Paris.'),
      write('country', '<h2>France</h2>', 'Show me a map of France.'),
      write('pendulum', '<h2>Pendulum</h2>', 'Draw a map of Paris and animate a pendulum.'),
      write('clock', '<h2>摆钟</h2>', '画一张杭州地图，再给我做一个摆钟。'),
    ]);
  });
  assert.match(mapPublication[0], /Missing geographic basemap/);
  assert.match(mapPublication[1], /Missing geographic basemap/);
  assert.deepEqual(mapPublication.slice(2, 4), [null, null]);
  assert.match(mapPublication[4], /no saved basemap/);
  assert.match(mapPublication[5], /Missing geographic basemap/);
  assert.equal(mapPublication[6], null);
  assert.match(mapPublication[7], /Missing geographic basemap/);
  assert.match(mapPublication[8], /shows only a city center/);
  assert.match(mapPublication[9], /cannot send stop names/);
  for (const issue of mapPublication.slice(10, 12)) assert.match(issue, /reader asked for a map/);
  assert.equal(mapPublication[12], null);
  assert.match(mapPublication[13], /Missing geographic basemap/);
  assert.deepEqual(mapPublication.slice(14, 17), [null, null, null]);
  assert.match(mapPublication[17], /Missing geographic basemap/);
  assert.match(mapPublication[18], /Missing geographic basemap/);
  assert.deepEqual(mapPublication.slice(19), [null, null]);
  // Exercise a headless producer's record through the ordinary FlowChat reader.
  const deferredUrl = new URL(url);
  deferredUrl.searchParams.set('deferred-request', 'Show me a map of Paris.');
  await page.evaluate(() => sessionStorage.setItem('visualize-test-fragment', '<h2>Paris</h2>'));
  await page.goto(deferredUrl);
  await page.waitForFunction(() => document.body.textContent.includes('Missing geographic basemap'));
  assert.equal(await page.$('[data-figure-check=passed]'), null);
  deferredUrl.searchParams.set('deferred-request', '画一张杭州地图，并给出行程表。');
  await page.goto(deferredUrl);
  const calendar = await open('<h2>行程表</h2><select><option>A</option><option>B</option></select>');
  assert.equal(await calendar.$eval('select', el => el.value), 'A');
  // Replace only the public tile service, leaving the projection worker and
  // publication reader intact. One vector tile containing a diagonal street.
  const hook = await page.evaluateOnNewDocument(() => {
    const NativeWorker = window.Worker;
    window.mapProjectionCount = 0;
    window.Worker = class extends NativeWorker {
      constructor(url, options) {
        if (!String(url).includes('regionalProjection')) { super(url, options); return; }
        window.mapProjectionCount++;
        const bootstrap = `
          const fetchAsset = self.fetch.bind(self);
          self.fetch = (url, opts) => new URL(url).hostname === 'tiles.versatiles.org'
            ? Promise.resolve(new Response(Uint8Array.from(atob('Ghl4AgoHc3RyZWV0cxIMGAIiCAkAAAqAQIBA'), c => c.charCodeAt(0))))
            : fetchAsset(url, opts);
          const pending = [];
          self.onmessage = event => pending.push(event);
          await import(${JSON.stringify(String(url))});
          for (const event of pending) self.onmessage(event);
        `;
        super(URL.createObjectURL(new Blob([bootstrap], { type: 'text/javascript' })), options);
      }
    };
  });
  const street = '<script type="application/json" data-visual-atlas="1">{"detail":"street","places":[{"name":"A","coordinates":[120.15,30.26]},{"name":"B","coordinates":[120.16,30.27]}]}</script>';
  await page.evaluate(source => sessionStorage.setItem('visualize-test-fragment', source), street);
  await page.reload();
  const consentButton = '[data-codex-figure] [role=status] button';
  await page.waitForSelector(consentButton);
  assert.equal(await page.evaluate(() => window.mapProjectionCount), 0);
  await page.click(consentButton);
  await page.waitForSelector('[data-figure-check=passed]');
  assert.equal(await page.evaluate(() => window.mapProjectionCount), 1);
  const restoredMap = await open(street);
  assert.ok(await restoredMap.$('path[data-street-layer="streets"]'));
  assert.equal(await page.evaluate(() => window.mapProjectionCount), 0);
  await page.evaluate(source => sessionStorage.setItem('visualize-test-fragment', source.replace('120.15,', '120.14,')), street);
  await page.reload();
  await page.waitForSelector(consentButton);
  assert.equal(await page.evaluate(() => window.mapProjectionCount), 0);
  await page.removeScriptToEvaluateOnNewDocument(hook.identifier);
  await page.goto(url);
  console.log('Passed deferred map intent, reader consent, offline street replay and revision isolation.');
  // A collapsed lake ring once covered the entire viewport, even though the
  // figure had both valid SVG paths and the right city labels.
  const deltaMap = await open('<script type="application/json" data-visual-atlas="1">{"places":["上海","杭州","苏州"],"language":"zh-CN"}</script>');
  const geography = await deltaMap.evaluate(() => {
    const land = document.querySelector('[data-map-layer="land"]');
    const lakes = document.querySelector('[data-map-layer="lakes"]');
    const point = (el, x, y) => new DOMPoint(Number(el.getAttribute(x)), Number(el.getAttribute(y)));
    const markers = [...document.querySelectorAll('[data-visual-marker] circle')];
    const label = name => [...document.querySelectorAll('[data-map-context]')].find(el => el.textContent === name);
    const lake = point(label('太湖'), 'x', 'y');
    const sea = point(label('东海'), 'x', 'y');
    return {
      cities: markers.length,
      onLand: markers.every(el => land.isPointInFill(point(el, 'cx', 'cy')) && !lakes.isPointInFill(point(el, 'cx', 'cy'))),
      taihu: lakes.isPointInFill(lake),
      sea: !land.isPointInFill(sea),
      fills: [land, lakes, markers[0]].map(el => getComputedStyle(el).fill),
    };
  });
  assert.deepEqual([geography.cities, geography.onLand, geography.taihu, geography.sea], [3, true, true, true]);
  assert.equal(new Set(geography.fills).size, 3);
  await verifyRepaint(deltaMap, '[data-map-layer="land"], [data-map-layer="lakes"], [data-visual-marker] circle, [data-visual-marker] text', 8);
  const kansai = await open('<script type="application/json" data-visual-atlas="1">{"places":[{"name":"Osaka","country":"JP"},{"name":"Kyoto","country":"JP"}]}</script>');
  assert.equal(await kansai.$$eval('[data-visual-marker] circle', els => els.length), 2);
  console.log('Passed offline regional atlas, water/land separation and city lookup in two regions.');
  const plainReply = new URL(url);
  const checkedScript = await page.evaluate(async dependency => {
    const { inspectWrittenFigure } = await import('/src/tools/visualize/backgroundInspection.ts');
    return inspectWrittenFigure({
      uri: 'openbitfun://current-session/artifacts/visualizations/v2/choice.html',
      source: '<output></output><script src="' + dependency + '"></script>',
    }, async () => undefined);
  }, dependency);
  assert.match(checkedScript.renderedSource, /data:application\/javascript;base64,/);
  const stateProbe = await open(checkedScript.renderedSource);
  assert.equal(await stateProbe.$eval('output', el => el.textContent), 'first');
  const replayState = await stateProbe.evaluate(async () => {
    await window.openai.setWidgetState({
      modelContent: { cityName: 'Tokyo' },
      privateContent: { cityId: 'tokyo', choice: 'saved', modelContent: 'collision' },
    });
    const state = window.openai.widgetState;
    return [state.cityId, state.modelContent, JSON.parse(JSON.stringify(state))];
  });
  assert.deepEqual(replayState, [
    'tokyo',
    { cityName: 'Tokyo' },
    { modelContent: { cityName: 'Tokyo' }, privateContent: { cityId: 'tokyo', choice: 'saved', modelContent: 'collision' } },
  ]);
  const reopenedState = await open(checkedScript.renderedSource);
  assert.equal(await reopenedState.evaluate(() => window.openai.widgetState.cityId), 'tokyo');
  assert.equal(await reopenedState.$eval('output', el => el.textContent), 'saved');
  assert.equal(scriptLoads, 1, 'replay must use the exact bytes verified at publication');
  console.log('Passed pending CSV deferral and saved selection replay with the script CDN disconnected.');
  plainReply.searchParams.set('plain-reply', '1');
  await page.goto(plainReply, { waitUntil: 'domcontentloaded' });
  const recovered = await open(
    '<button id="counter" type="button">1</button><script>const counter=document.getElementById("counter");counter.onclick=()=>counter.textContent="2";</script>',
  );
  await recovered.waitForSelector('#counter', { visible: true });
  await recovered.click('#counter');
  assert.equal(await recovered.$eval('#counter', (button) => button.textContent), '2');
  assert.equal(await page.$$eval('[data-codex-figure]', (figures) => figures.length), 1);
  console.log('Passed recovery of an interactive figure when the model returned plain text.');
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  const paints = await open(`
    <style>#chart { height:180px }</style>
    <h2>Theme</h2><button id="step" class="btn btn-primary">1</button><div id="chart"></div>
    <script>
      const step = document.querySelector('#step');
      step.onclick = () => step.textContent = '2';
      const chart = echarts.init(document.querySelector('#chart'), null, { renderer: 'svg' });
      chart.setOption({ animation:false, legend:{}, xAxis:{type:'category',data:['A','B']}, yAxis:{},
        series:[{type:'bar',name:'One',data:[1,2]},{type:'bar',name:'Two',data:[1,2]}] });
      window.firstPaint = [window.openai.theme, getComputedStyle(document.body).color,
        getComputedStyle(document.documentElement).getPropertyValue('--viz-series-1').trim()];
      chart.setOption({series:[{data:[3,5]}]});
      chart.dispatchAction({type:'legendUnSelect',name:'Two'});
    </script>`);
  await paints.click('#step');
  await page.mouse.move(700, 600);
  async function checkPaint() {
    const expected = await page.$eval('[data-codex-figure]', host => {
      const probe = host.appendChild(document.createElement('span'));
      const values = ['content-primary', 'content-secondary', 'accent-default', 'action-primary-background', 'action-primary-content', 'action-primary-hover'].map(token => {
        probe.style.color = 'var(--openbitfun-color-' + token + ')';
        return getComputedStyle(probe).color;
      });
      probe.remove();
      return values;
    });
    await paints.waitForFunction(([ink, label, series, button, buttonInk, hover]) => {
      const step = document.querySelector('#step');
      // zrender splits SVG paint into fill and fill-opacity.
      const axisLabel = [...document.querySelectorAll('#chart text')].find(el => el.textContent === 'A');
      if (!axisLabel) return false;
      const pixel = document.createElement('canvas').getContext('2d');
      function rgba(color, opacity = 1) {
        pixel.clearRect(0, 0, 1, 1);
        pixel.fillStyle = color;
        pixel.globalAlpha = opacity;
        pixel.fillRect(0, 0, 1, 1);
        return pixel.getImageData(0, 0, 1, 1).data;
      }
      const axisStyle = getComputedStyle(axisLabel);
      const actualLabel = rgba(axisStyle.fill, Number(axisStyle.fillOpacity));
      const expectedLabel = rgba(label);
      return getComputedStyle(document.documentElement).backgroundColor === 'rgba(0, 0, 0, 0)' &&
        getComputedStyle(document.body).color === ink && getComputedStyle(step).backgroundColor === (step.matches(':hover') ? hover : button) &&
        getComputedStyle(step).color === buttonInk &&
        [...document.querySelectorAll('#chart path[d]')].some(el => getComputedStyle(el).fill === series && el.getBBox().height > 5) &&
        actualLabel.every((value, i) => Math.abs(value - expectedLabel[i]) <= 1);
    }, {}, expected);
    const state = await paints.evaluate(() => {
      const option = echarts.getInstanceByDom(document.querySelector('#chart')).getOption();
      return [document.querySelector('#step').textContent, option.legend[0].selected.Two, option.series[0].data];
    });
    assert.deepEqual(state, ['2', false, [3,5]]);
    return expected;
  }
  const first = await checkPaint();
  assert.deepEqual(await paints.evaluate(() => window.firstPaint), [
    narrow ? 'dark' : 'light', first[0],
    await page.$eval('[data-codex-figure]', el => getComputedStyle(el).getPropertyValue('--openbitfun-color-accent-default').trim()),
  ]);
  await page.evaluate(async () => {
    const { appearanceRuntime, buildBuiltinAppearance, openBitFunMidnightPalette } = await import('/src/infrastructure/appearance/index.ts');
    await appearanceRuntime.applyPackage(buildBuiltinAppearance(openBitFunMidnightPalette));
    document.documentElement.dataset.colorScheme = 'dark';
  });
  const midnight = await checkPaint();
  assert.notDeepEqual(midnight, first);
  await page.evaluate(async () => {
    const { skinRuntime } = await import('/src/infrastructure/visual-skin/index.ts');
    const skin = { manifest:{schema:'openbitfun.visual-skin',schemaVersion:1,id:'chart-paint',name:'Chart paint',version:'1.0.0'},
      modes:{dark:{tokens:{'color.accent.default':'#7dd3a4','color.action.primary.background':'#8fd4bf','color.action.primary.content':'#17312f'}}} };
    await skinRuntime.importPackage(new TextEncoder().encode(JSON.stringify(skin)).buffer);
  });
  assert.notDeepEqual(await checkPaint(), midnight);
  await page.evaluate(async () => (await import('/src/infrastructure/visual-skin/index.ts')).skinRuntime.deactivate());
  assert.deepEqual(await checkPaint(), midnight);
  console.log('Passed themed first paint, transparent chat background, button contrast pair and live chart paint with updated data and selection preserved.');
  if (process.argv.includes('--bundled') || !existsSync(new URL('assets/visualize.html', root))) {
    const frame = await open(`
      <div role="tablist"><button role="tab" aria-controls="plot" aria-selected="true">Chart</button>
      <button role="tab" aria-controls="settings">Options</button></div>
      <section id="plot" style="height:200px"></section><section id="settings" hidden><output></output></section>
      <script>
        const chart = echarts.init(document.getElementById('plot'), null, { renderer: 'svg' });
        chart.setOption({ xAxis: { type: 'category', data: ['A','B'] }, yAxis: {}, series: [{ type: 'bar', data: [4,7] }] });
        const state = { zoom: 2 };
        const show = () => document.querySelector('output').textContent = String(state.zoom);
        show();
        new Tweak({container: document.getElementById('settings'), onChange: show}).addSlider(state, 'zoom', {min: 1, max: 5});
      </script>`);
    await frame.waitForFunction(() => [...document.querySelectorAll('#plot svg path[d]')].filter(path => {
      const bounds = path.getBoundingClientRect();
      return bounds.width > 0 && bounds.height > 0;
    }).length >= 2);
    await frame.click('[aria-controls=settings]');
    await frame.waitForSelector('#settings output', { visible: true });
    await page.click('.visual-codex__menu summary');
    await page.locator('text/Adjust design').click();
    await page.focus('fieldset input[type=range]');
    await page.keyboard.press('End');
    await frame.waitForFunction(() => document.querySelector('output').textContent === '5');
    console.log('Passed bundled charts, tabs, design controls and host state without the reference kit.');
    return;
  }
  const original = process.env.OPENBITFUN_REFERENCE_FRAGMENT;
  if (original) {
    const html = await readFile(original, 'utf8');
    const frame = await open(html);
    if (process.env.OPENBITFUN_REFERENCE_SCREENSHOT) {
      await page.setViewport({ width: 810, height: 720 });
      await page.locator('[data-codex-figure]').wait();
      await (
        await page.$('[data-codex-figure]')
      ).screenshot({ path: process.env.OPENBITFUN_REFERENCE_SCREENSHOT });
    }
    await frame.locator('button[data-mode=creative]').click();
    await frame.waitForFunction(
      () => document.querySelector('[data-mode=creative]')?.getAttribute('aria-pressed') === 'true',
    );
    await frame.locator('#obf-tab-layers').click();
    assert.equal(await frame.$eval('#obf-layers', (e) => e.hidden), false);
    await frame.locator('#obf-tab-remote').click();
    await frame.locator('button[data-remote=dispatch]').click();
    await page.waitForFunction(() =>
      Object.values(localStorage).some((value) => {
        try {
          const state = JSON.parse(value)?.modelContent;
          return state?.mode === 'creative' && state?.remote === 'dispatch';
        } catch {
          return false;
        }
      }),
    );
    const replay = await open(html);
    await replay.waitForFunction(
      () => document.querySelector('[data-remote=dispatch]')?.getAttribute('aria-pressed') === 'true',
    );
    await replay.locator('#obf-tab-product').click();
    assert.equal(await replay.$eval('[data-mode=creative]', (e) => e.getAttribute('aria-pressed')), 'true');
    console.log('Passed reference fragment: all three tabs, modes, remote routes and state after replay.');
  }
  if (!process.env.OPENBITFUN_CHART_ONLY) {
    const themed = await open(`
      <button id="toggle" class="btn" aria-pressed="false">Day 2</button>
      <svg width="80" height="30"><path d="M1 15H79" stroke="var(--viz-series-1)" /></svg>
      <main></main><script type="module">
        import { mountChart } from "https://persistent.oaistatic.com/plugins/visualize/charts/1.6.5/element.js";
        mountChart(document.querySelector('main'), {id:"temperature",title:"Temperature",spec:{type:"line",x:"day",y:"value"},
          columns:["day","value"],rows:[[1,4],[2,9],[3,6]]});
      </script><script>document.querySelector('button').onclick=e=>e.target.setAttribute('aria-pressed','true')</script>`);
    await themed.click('#toggle');
    const expectedColors = () => page.$eval('[data-codex-figure]', host => {
      const probe = document.createElement('span');
      host.append(probe);
      const colors = ['surface-canvas', 'content-primary', 'accent-default'].map(token => {
        probe.style.color = 'var(--openbitfun-color-' + token + ')';
        return getComputedStyle(probe).color;
      });
      probe.remove();
      return colors;
    });
    async function checkAppearance() {
      const expected = await expectedColors();
      await themed.waitForFunction(([paper, ink, series]) => {
        const chart = document.querySelector('main > *')?.shadowRoot;
        const plot = chart && [...chart.querySelectorAll('*')].find(el => el.shadowRoot)?.shadowRoot;
        const line = plot?.querySelector('.recharts-line-curve');
        return line && getComputedStyle(line).stroke === series
          && getComputedStyle(document.documentElement).backgroundColor === 'rgba(0, 0, 0, 0)'
          && getComputedStyle(document.body).color === ink
          && getComputedStyle(document.querySelector('svg path')).stroke === series;
      }, {}, expected);
      assert.equal(await themed.$eval('#toggle', el => el.getAttribute('aria-pressed')), 'true');
      return expected;
    }
    const initialColors = await checkAppearance();
    await page.evaluate(async () => {
      const { appearanceRuntime, buildBuiltinAppearance, openBitFunMidnightPalette } =
        await import('/src/infrastructure/appearance/index.ts');
      await appearanceRuntime.applyPackage(buildBuiltinAppearance(openBitFunMidnightPalette));
      document.documentElement.dataset.colorScheme = 'dark';
    });
    const midnightColors = await checkAppearance();
    const skinArchive = process.env.OPENBITFUN_VISUALIZE_TEST_SKIN;
    const skin = skinArchive
      ? await readFile(skinArchive)
      : Buffer.from(JSON.stringify({
          manifest: { schema: 'openbitfun.visual-skin', schemaVersion: 1, id: 'figure-test', name: 'Figure test', version: '1.0.0' },
          modes: { dark: { tokens: { 'color.surface.canvas': '#192b36', 'color.content.primary': '#f4f8fb', 'color.accent.default': '#7dd3a4' } } },
        }));
    await page.evaluate(async (encoded) => {
      const { skinRuntime } = await import('/src/infrastructure/visual-skin/index.ts');
      const bytes = Uint8Array.from(atob(encoded), (c) => c.charCodeAt(0));
      await skinRuntime.importPackage(bytes.buffer);
    }, skin.toString('base64'));
    assert.notDeepEqual(await checkAppearance(), midnightColors);
    await page.evaluate(async () => {
      const { skinRuntime } = await import('/src/infrastructure/visual-skin/index.ts');
      await skinRuntime.deactivate();
    });
    assert.deepEqual(await checkAppearance(), midnightColors);
    if (!narrow) assert.notDeepEqual(initialColors, midnightColors);
    console.log('Passed live Appearance and imported skin changes on map strokes, chart marks, text and background with selection preserved.');
    const itinerary = await open(`
      <style>#trip [role="tabpanel"] { display: flex }</style>
      <div id="trip">
        <div class="nav nav-pills" role="tablist" aria-label="Itinerary">
          <button id="day-1" class="nav-link" role="tab" aria-controls="panel-1" aria-selected="true">Day 1</button>
          <button id="day-2" class="nav-link" role="tab" aria-controls="panel-2">Day 2</button>
          <button id="day-3" class="nav-link" role="tab" aria-controls="panel-3">Day 3</button>
        </div>
        <section id="panel-1" role="tabpanel" aria-labelledby="day-1">First day</section>
        <section id="panel-2" role="tabpanel" aria-labelledby="day-2" hidden>Second day</section>
        <section id="panel-3" role="tabpanel" aria-labelledby="day-3" hidden>Third day</section>
        <label><input type="checkbox" id="filter" data-tooltip="Choose categories">Show filter</label>
        <div id="filter-options" style="display:grid" hidden>Filter options</div>
      </div>`);
    await itinerary.evaluate(() => {
      const filter = document.getElementById('filter');
      filter.onchange = () => { document.getElementById('filter-options').hidden = !filter.checked; };
      for (const panel of document.querySelectorAll('[role="tabpanel"]')) {
        new Tweak({ container: panel, onChange() {} }).addSlider({ size: 3 }, 'size', { min: 1, max: 10 });
      }
    });
    const filterVisible = () => itinerary.$eval('#filter-options', (el) => el.getClientRects().length > 0);
    assert.equal(await filterVisible(), false);
    await itinerary.click('#filter');
    assert.equal(await filterVisible(), true);
    await itinerary.click('#filter');
    assert.equal(await filterVisible(), false);
    await itinerary.focus('#day-1');
    await page.keyboard.press('Tab');
    await itinerary.focus('#filter');
    await itinerary.waitForFunction(() => {
      const tip = document.querySelector('.tooltip');
      return tip?.textContent === 'Choose categories' && getComputedStyle(tip).visibility === 'visible';
    });
    await page.keyboard.press('Escape');
    await itinerary.waitForFunction(() => !document.querySelector('.tooltip'));
    const visiblePanels = () => itinerary.$$eval('[role="tabpanel"]', (panels) =>
      panels.filter((panel) => panel.getClientRects().length > 0).map((panel) => panel.id),
    );
    assert.deepEqual(await visiblePanels(), ['panel-1']);
    await itinerary.click('#day-2');
    assert.deepEqual(await visiblePanels(), ['panel-2']);
    await page.locator('.visual-codex__menu summary').click();
    await page.locator('text/Adjust design').click();
    const onlyDesignGroup = (id) => page.waitForFunction((expected) => {
      const legends = [...document.querySelectorAll('fieldset legend')].filter(el => el.getClientRects().length);
      return legends.length === 1 && legends[0].textContent === expected;
    }, {}, id);
    await onlyDesignGroup('panel-2');
    await itinerary.focus('#day-2');
    await page.keyboard.press('ArrowRight');
    assert.deepEqual(await visiblePanels(), ['panel-3']);
    await onlyDesignGroup('panel-3');
    console.log('Passed visible tab content, conditional grid and matching design controls.');
    const mock = await open(
      '<!-- vis-bindings\n{"counts":{"type":"json","value":{"n":7}}}\n-->\n' +
        '<style>#designs [data-variant]{display:grid}</style><div class="viz-carousel" id="designs" aria-label="Designs"><section data-variant="Compact"><div id="editor" aria-label="Layout"><output></output></div></section><section data-variant="Spacious" hidden>Spacious layout</section></div><p id="count"></p>' +
        '<script>const state={size:3,dense:false,color:"#3366ff",align:"left"};const el=document.getElementById("editor");const show=()=>el.firstChild.textContent=JSON.stringify(state);show();const tw=new Tweak({container:el,onChange:show});tw.addSlider(state,"size",{min:1,max:10});tw.addToggle(state,"dense");tw.addColorPicker(state,"color");tw.addSelect(state,"align",{options:["left","right"]});window.openai.visualization.onData("counts",v=>document.getElementById("count").textContent=v.n);</script>',
    );
    assert.equal(await mock.$eval('#count', (e) => e.textContent), '7');
    await page.click('.visual-codex__menu summary');
    await page.locator('text/Adjust design').click();
    await page.waitForSelector('fieldset select');
    await page.focus('fieldset input[type=range]');
    await page.keyboard.press('End');
    await page.click('fieldset input[type=checkbox]');
    await page.select('fieldset select', 'right');
    await page.$eval('fieldset input[type=color]', (e) => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(e, '#112233');
      e.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await mock.waitForFunction(
      () =>
        document.querySelector('output').textContent ===
        JSON.stringify({ size: 10, dense: true, color: '#112233', align: 'right' }),
    );
    await page.locator('text/Preview original').click();
    await mock.waitForFunction(() => JSON.parse(document.querySelector('output').textContent).size === 3);
    await page.locator('text/Preview original').click();
    await mock.waitForFunction(() => JSON.parse(document.querySelector('output').textContent).size === 10);
    const visibleVariants = () => mock.$$eval('[data-variant]', (panels) =>
      panels.filter(el => el.getClientRects().length).map(el => el.dataset.variant),
    );
    await mock.click('.viz-carousel-next');
    assert.deepEqual(await visibleVariants(), ['Spacious']);
    await page.waitForFunction(() => !document.querySelector('fieldset'));
    await mock.click('.viz-carousel-previous');
    assert.deepEqual(await visibleVariants(), ['Compact']);
    await page.waitForSelector('fieldset');
    await page.locator('text/Reset changes').click();
    await mock.waitForFunction(() => JSON.parse(document.querySelector('output').textContent).size === 3);
    await mock.click('.viz-carousel-picker summary');
    await mock.locator('.viz-carousel-options button:last-child').click();
    assert.deepEqual(await visibleVariants(), ['Spacious']);
    const sourcePanel = '.visual-codex > section[aria-label]';
    assert.equal(await page.$eval(sourcePanel, el => el.getClientRects().length), 0);
    for (const opening of [true, false]) {
      await page.locator('.visual-codex__menu summary').click();
      await page.locator('.visual-codex__menu ::-p-text(Sources)').click();
      assert.equal(await page.$eval(sourcePanel, el => el.getClientRects().length > 0), opening);
      if (opening) assert.match(await page.$eval(sourcePanel, el => el.innerText), /counts/);
    }
    console.log('Passed native design controls, variant carousel, reset and synchronous onData.');
    const dashboard = await open(
      '<!-- vis-bindings\n{"counts":{"type":"json","value":{"n":7}}}\n-->\n' +
      '<section id="orders" data-dashboard-block aria-label="Orders" sources="counts"><h2>Orders</h2></section>' +
      '<dash-sources></dash-sources>',
    );
    await dashboard.locator('dash-sources >>> button[aria-haspopup="dialog"]').click();
    await page.waitForFunction(selector => {
      const panel = document.querySelector(selector);
      return panel?.getClientRects().length && panel.innerText.includes('counts');
    }, {}, sourcePanel);
    await page.locator('.visual-codex__menu summary').click();
    await page.locator('text/Comment on a block').click();
    await dashboard.waitForFunction(() => window.openai.sandboxDocumentState?.dashboardAnnotateMode);
    await dashboard.click('#orders h2');
    const note = '[data-codex-figure] label textarea';
    await page.waitForSelector(note, { visible: true });
    assert.match(await page.$eval(note, el => el.closest('label').innerText), /Orders/);
    await page.type(note, 'Check reported count');
    await page.evaluate(() => {
      window.addEventListener('fill-chat-input', event => {
        document.body.dataset.annotationDraft = event.detail.message;
      }, { once: true });
    });
    await page.locator('text/Ask about selection').click();
    await page.waitForFunction(() => document.body.dataset.annotationDraft?.includes('Orders\nCheck reported count'));
    console.log('Passed dashboard source inspection and follow-up draft for the selected block.');
    const guides = await Promise.all(
      ['calendar', 'waterfall', 'flamechart', 'stacktrace'].map(async (name) => {
        const guide = await readFile(new URL('widgets/' + name + '.md', root), 'utf8');
        return guide.match(/```html\n([\s\S]*?)\n```/)[1];
      }),
    );
    const widgetFrame = await open(guides.join('\n'));
    await widgetFrame.waitForFunction(
      () =>
        ['viz-calendar', 'viz-waterfall', 'viz-flame-chart', 'viz-stack-trace'].every((tag) => {
          const el = document.querySelector(tag);
          return el?.matches(':defined') && (el.shadowRoot?.textContent ?? el.textContent).trim().length > 0;
        }),
      { timeout: 60000 },
    );
    await verifyRepaint(widgetFrame, 'viz-calendar, viz-waterfall, viz-flame-chart, viz-stack-trace', 4);
    await widgetFrame.evaluate(() => {
      const calendar = document.querySelector('viz-calendar');
      const detail = document.body.appendChild(document.createElement('output'));
      detail.id = 'event-detail';
      calendar.setAttribute('interactive', '');
      calendar.addEventListener('eventselect', (event) => { detail.textContent = event.detail.event.title; });
    });
    await widgetFrame.locator('viz-calendar >>> button[aria-label^="Focus time"]').click();
    assert.equal(await widgetFrame.$eval('#event-detail', (el) => el.innerText), 'Focus time');
    const clickInWidget = (selector) => widgetFrame.locator(selector).click();
    const widgetText = (host, text, expected = true) => widgetFrame.waitForFunction((selector, needle, present) => {
      const section = document.querySelector(selector)?.shadowRoot?.querySelector('section');
      return section?.innerText.includes(needle) === present;
    }, {}, host, text, expected);
    await clickInWidget('#request >>> button[title="SELECT orders"]');
    await widgetText('#request', 'Zoom to span');
    await widgetText('#request', '80 ms');
    await clickInWidget('#request >>> button[aria-label^="Collapse GET /api"]');
    await widgetText('#request', 'SELECT orders', false);
    await clickInWidget('#request >>> button[aria-label^="Expand GET /api"]');
    await widgetText('#request', 'SELECT orders');
    await clickInWidget('#profile >>> button[aria-label^="layoutRows"]');
    await widgetText('#profile', '2 ms total · 2 ms self');
    await clickInWidget('#profile >>> button[data-action="zoom-in"]');
    await widgetText('#profile', '750 µs');
    await clickInWidget('#failure >>> button[title^="show_order"]');
    await widgetText('#failure', "return order['total']", false);
    // Switching stack frames can change the panel's height. Exercise keyboard
    // selection too, with focus on the target in its updated shadow tree.
    const totalFrame = await widgetFrame.waitForSelector('#failure >>> button[title^="order_total"]', { visible: true });
    await totalFrame.focus();
    await totalFrame.press('Enter');
    await widgetText('#failure', "return order['total']");
    console.log('Passed calendar event selection, waterfall drill-down, flame zoom and stack-frame source switching.');
    const map = await open(
      '<svg viewBox="0 0 640 360" aria-label="World"></svg><script type="module">import atlas from "https://esm.sh/@d3-maps/atlas@1.0.0/world/countries/countries-110m";import {feature} from "https://esm.sh/topojson-client@3.1.0";import {geoEqualEarth,geoPath} from "https://esm.sh/d3-geo@3.1.1";const fc=feature(atlas,atlas.objects.features);const project=geoPath(geoEqualEarth().fitSize([640,360],fc));for(const f of fc.features){const p=document.createElementNS("http://www.w3.org/2000/svg","path");p.setAttribute("d",project(f));document.querySelector("svg").append(p);}</script>',
    );
    await map.waitForFunction(() => document.querySelectorAll('svg path[d]').length > 100);
    // This exact D3 patch appeared in an actual regional itinerary figure.
    const region = await open('<svg viewBox="0 0 400 200"><path /></svg><script type="module">import {geoMercator,geoPath} from "https://esm.sh/d3-geo@3.1.0";const path=geoPath(geoMercator().center([121,31]).scale(6000));document.querySelector("path").setAttribute("d",path({type:"LineString",coordinates:[[121.47,31.23],[120.62,31.30]]}));</script>');
    await region.waitForFunction(() => document.querySelector('svg path').getTotalLength() > 80);
    console.log('Passed world atlas, TopoJSON conversion, and exact regional D3 projection.');
  }
  const specs = [
    ...['line', 'area', 'sparkline', 'bar', 'scatter', 'boxPlot'].map((type) => ({ type, x: 'x', y: 'y' })),
    ...['pie', 'rankedList', 'funnel'].map((type) => ({ type, category: 'group', value: 'y' })),
    { type: 'histogram', input: 'raw', x: 'x' },
    { type: 'heatmap', x: 'group', y: 'x', value: 'y' },
    { type: 'sankey', stages: ['group', 'target'], value: 'y' },
    { type: 'waterfall', x: 'group', y: 'y', beginning: 0 },
  ];
  const chart = await open(`
    <button id="sample">Change dataset</button><main></main>
    <script type="module">
      import { mountChart } from "https://persistent.oaistatic.com/plugins/visualize/charts/1.8.1/element.js";
      const rows = [["A","C",1,5],["B","C",2,3],["E","D",3,2]];
      const alternateRows = [["Z","Q",13,39],["Y","Q",19,22],["W","P",25,8],["V","P",61,1]];
      const charts = ${JSON.stringify(specs)}.map(spec => {
        const container = document.createElement("section");
        document.querySelector("main").append(container);
        return mountChart(container, {id:spec.type, title:spec.type, spec, columns:["group","target","x","y"], rows});
      });
      let alternate = false;
      document.getElementById("sample").onclick = () => {
        alternate = !alternate;
        for (const chart of charts) chart.config = {...chart.config, rows:alternate ? alternateRows : rows};
      };
    </script>`);
  // Compare rendered geometry or ranked values, independent of the component's config object.
  const readPlots = await chart.evaluateHandle(() => () => Array.from(document.querySelectorAll('main>section>*'), host => {
    const renderer = [...host.shadowRoot.querySelectorAll('*')].find(el => el.shadowRoot)?.shadowRoot;
    // At narrow widths the funnel displays its stages as a list and hides the ribbon.
    const plots = renderer?.querySelectorAll('.recharts-surface,.chart-ranked-list,.chart-funnel-ribbon,.chart-funnel-stages');
    const plot = Array.from(plots ?? []).find(element => element.getBoundingClientRect().width > 0);
    if (!plot) return null;
    if (plot.matches('[role="list"],ol')) return plot.innerText;
    const marks = [...plot.querySelectorAll('path,rect,circle,polygon,line,text')].filter(el => !el.closest('defs'));
    if (!marks.length) return null;
    return JSON.stringify(marks.map(el => [
      ...['d','points','cx','cy','height','x','y'].map(attr => el.getAttribute(attr)), el.textContent,
    ]));
  }));
  await chart.waitForFunction((read, count) => {
    const plots = read();
    return plots.length === count && plots.every(Boolean);
  }, {}, readPlots, specs.length);
  await verifyRepaint(chart, 'main > section', specs.length);
  const initialPlots = await chart.evaluate(read => read(), readPlots);
  for (const restoring of [false, true]) {
    await chart.click('#sample');
    await chart.waitForFunction((read, previous, restore) => {
      const plots = read();
      return plots.length === previous.length && plots.every((plot, i) => plot && (plot === previous[i]) === restore);
    }, {}, readPlots, initialPlots, restoring);
  }
  await readPlots.dispose();
  console.log('Passed visible rendering, data changes and restoration for all 13 chart families.');
}
try {
  await testHost();
} finally {
  await browser.close();
}
