import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFile } from 'node:fs/promises';

const script = await readFile(new URL('../src/routes/page_auth_client.bundle.js', import.meta.url), 'utf8');

async function runLogin({ state, status = 200, smsStatus = 202, errorCode } = {}) {
  let submit;
  let sendCode;
  let redirect;
  const calls = [];
  const intervals = [];
  const button = { disabled: false, textContent: 'Sign in with phone number' };
  const sendButton = {
    disabled: false,
    textContent: 'Send code',
    addEventListener: (_, callback) => { sendCode = callback; },
  };
  const error = { hidden: true, textContent: '' };
  const phone = { value: ' 13800000002 ' };
  const code = { value: '123456' };
  const form = { dataset: { pageLoginState: state }, addEventListener: (_, callback) => { submit = callback; } };
  const elements = {
    '[data-page-login-form]': form,
    '[data-page-login-submit]': button,
    '[data-page-login-send-code]': sendButton,
    '[data-page-login-error]': error,
    '[data-page-login-phone]': phone,
    '[data-page-login-code]': code,
  };
  const context = {
    document: { querySelector: (selector) => elements[selector] },
    navigator: { language: 'en' },
    URL, Error, Date,
    setTimeout: (callback) => { callback(); return 0; },
    window: {
      location: { pathname: state ? '/v/1.0.0/api/page-auth/login' : '/v/1.0.0/p/alice/demo', search: '?q=1', replace: (value) => { redirect = value; } },
      setInterval: (callback, ms) => { intervals.push({ callback, ms }); return intervals.length; },
      clearInterval: () => {},
    },
    fetch: async (url, options) => {
      const body = JSON.parse(options.body);
      calls.push({ url, body });
      if (url.endsWith('/api/auth/local/sms/send')) {
        if (smsStatus !== 202) return { ok: false, status: smsStatus, json: async () => ({ error: 'please wait before requesting another code', retry_after_secs: 42 }) };
        return { ok: true, status: 202, json: async () => ({ status: 'sent' }) };
      }
      if (url.endsWith('/api/auth/local/login')) {
        if (errorCode) return { ok: false, status: errorCode.status, json: async () => ({ error: errorCode.error }) };
        assert.deepEqual(body, { phone: '13800000002', code: '123456' });
        return { ok: true, status: 200, json: async () => ({ accessToken: 'local-access-token', refreshToken: 'local-refresh-token' }) };
      }
      if (url.endsWith('/page-auth/login')) return { ok: true, status, json: async () => ({ redirect_to: state ? 'https://pages.example/callback?code=one-time' : '/p/alice/demo?q=1' }) };
      throw new Error(`Unexpected URL ${url}`);
    },
  };
  vm.runInNewContext(script, context);
  return {
    calls, error, button, sendButton, intervals, form,
    // The redirect happens inside the async submit handler; read it live.
    get redirect() { return redirect; },
    submitLogin: () => submit({ preventDefault() {} }),
    clickSend: () => sendCode(),
  };
}

test('code sign-in exchanges locally then submits only the verified identity for Page access', async () => {
  const page = await runLogin();
  await page.clickSend();
  await page.submitLogin();
  const calls = page.calls;
  // The verification code request precedes everything and carries the phone trimmed.
  assert.equal(calls[0].url.endsWith('/api/auth/local/sms/send'), true);
  assert.deepEqual(calls[0].body, { phone: '13800000002' });
  assert.deepEqual(calls[1].body, { phone: '13800000002', code: '123456' });
  assert.deepEqual(calls[2].body, { access_token: 'local-access-token', return_to: '/p/alice/demo?q=1', path_prefix: '/v/1.0.0' });
  assert.equal(page.redirect, '/v/1.0.0/p/alice/demo?q=1');
  assert.ok(page.intervals.length >= 1, 'a successful send starts the resend cooldown');
});

test('isolated Page sign-in retains the one-time login state and callback origin', async () => {
  const page = await runLogin({ state: 'login-state' });
  await page.clickSend();
  await page.submitLogin();
  assert.deepEqual(page.calls[2].body, { access_token: 'local-access-token', state: 'login-state' });
  assert.equal(page.redirect, 'https://pages.example/callback?code=one-time');
});

test('a rejected code never submits Page access and stays retryable', async () => {
  const page = await runLogin({ errorCode: { status: 401, error: 'verification code incorrect' } });
  await page.clickSend();
  await page.submitLogin();
  assert.equal(page.calls.some((call) => call.url.endsWith('/page-auth/login')), false);
  assert.equal(page.redirect, undefined);
  assert.equal(page.error.hidden, false);
  assert.match(page.error.textContent, /verification code was not accepted/);
  assert.equal(page.button.disabled, false);
});

test('an SMS cooldown answer surfaces the retry window without starting the client countdown', async () => {
  const page = await runLogin({ smsStatus: 429 });
  await page.clickSend();
  assert.equal(page.calls.length, 1);
  assert.equal(page.error.hidden, false);
  assert.match(page.error.textContent, /42 seconds/);
  assert.equal(page.sendButton.disabled, false, 'a refused send must not lock the button for the client cooldown');
  assert.equal(page.intervals.length, 0);
});
