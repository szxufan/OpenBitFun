import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const deps = createRequire(new URL('../tests/e2e/package.json', import.meta.url));
const puppeteer = createRequire(deps.resolve('@wdio/cli'))('puppeteer-core');
const url = process.env.OPENBITFUN_GENUI_TEST_URL;
if (!url) throw Error('Set OPENBITFUN_GENUI_TEST_URL to the Vite /tests/e2e/generative-ui.html fixture.');
const browser = await puppeteer.launch({
  headless: true,
  ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : { channel: 'chrome' }),
});
try {
  const page = await browser.newPage(),
    errors = [];
  const providerRequests = [];
  page.on('request', request => {
    if (/^\/api\/(?:wiki|image-search|street-route)(?:\/|$)/.test(new URL(request.url()).pathname)) {
      providerRequests.push(request.url());
    }
  });
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.setViewport({ width: 360, height: 740 });
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  async function verify() {
    await page.waitForFunction(() =>
      document.querySelector('[data-gen-ui-app="ledger"]')?.textContent.includes('47'),
    );
    const old = await page.waitForSelector('[data-gen-ui-app="saved-openui"]');
    assert.match(await old.evaluate((el) => el.textContent), /Stored OpenUI text/);
    assert.equal(await old.$eval('button', (e) => e.matches(':disabled')), true);
    assert.ok(await old.$('[role="img"][aria-label="Archived lighthouse"]'));
    const nested = await old.waitForSelector('iframe[title="Saved note"]');
    await nested.scrollIntoView();
    assert.equal(await nested.evaluate(el => el.getAttribute('sandbox')), '');
    const nestedDoc = await nested.contentFrame();
    await nestedDoc.waitForFunction(() => document.body.textContent.includes('Saved nested note'));
    assert.equal(await nestedDoc.$('script'), null);
    const frame = await page.waitForSelector('[data-gen-ui-app="saved-html"] iframe');
    assert.equal(await frame.evaluate((el) => el.sandbox.contains('allow-scripts')), false);
    const doc = await frame.contentFrame();
    await doc.waitForFunction(() => document.body.textContent.includes('Stored HTML note'));
    for (const [name, source] of [
      ['Historic phone', 'Archived phone content'],
      ['Future format', 'Preserved unknown content'],
    ]) {
      const summary = await page.waitForFunction(
        (t) => [...document.querySelectorAll('summary')].find((e) => e.textContent.includes(t)),
        {},
        name,
      );
      await summary.asElement().focus();
      await page.keyboard.press('Enter');
      await page.waitForFunction((el, text) => el.parentElement.open && el.parentElement.innerText.includes(text), {}, summary, source);
    }
    assert.equal(await page.$('[data-openbitfun-state="repairRequired"]'), null);
  }
  await verify();
  await page.reload();
  await verify();
  for (const deferred of [false, true]) {
    const next = new URL(url);
    next.searchParams.set('streaming', '');
    if (deferred) next.searchParams.set('deferred', '');
    await page.goto(next.href);
    for (let i = 1; i <= 4; i++) {
      await page.waitForSelector('[data-testid="stream-advance"]');
      await page.click('[data-testid="stream-advance"]');
      await page.waitForSelector('[data-stage="' + i + '"]');
    }
    await page.waitForFunction(() =>
      document.querySelector('[data-gen-ui-app]')?.textContent.includes('Streaming source'),
    );
  }
  assert.deepEqual(errors, []);
  assert.deepEqual(providerRequests, []);
  console.log(
    'Passed archived OpenUI and OpenBitFun views, stored state, isolated HTML, unsupported source, reload, and direct/deferred peer stream replay.',
  );
} finally {
  await browser.close();
}
