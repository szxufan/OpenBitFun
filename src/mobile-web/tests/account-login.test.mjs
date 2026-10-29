import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

async function loadSource(relativePath, imports = {}) {
  const source = await readFile(new URL(relativePath, import.meta.url), 'utf8');
  let code = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.React },
  }).outputText;
  code = code.replace(/from (['"])([^'"]+)\1/g, (_, quote, specifier) => (
    `from ${JSON.stringify(imports[specifier] ?? import.meta.resolve(specifier))}`
  ));
  return { url: `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`, source };
}

const links = await loadSource('../src/services/pairingLink.ts');
const selection = await loadSource('../src/services/accountDeviceSelection.ts');
const { selectAccountDevice } = await import(selection.url);
// The shared build module reads the workspace version from package.json; the
// transpile loader cannot import JSON, so hand it an equivalent JS module.
const workspacePackage = JSON.parse(await readFile(new URL('../../../package.json', import.meta.url), 'utf8'));
const packageJsonStub = `data:text/javascript;base64,${Buffer.from(
  `export const version = ${JSON.stringify(workspacePackage.version)};`,
).toString('base64')}`;
const clientBuild = await loadSource('../../shared/relay-transport/ClientBuild.ts', { '../../../package.json': packageJsonStub });
const { CLIENT_PROTOCOL_VERSION, CLIENT_VERSION } = await import(clientBuild.url);
const clientBuildImports = { '../../../shared/relay-transport/ClientBuild': clientBuild.url };
const offline = { device_id: 'desktop-a', device_name: 'Offline desktop', online: false };
const online = { device_id: 'desktop-b', device_name: 'Online desktop', online: true };
const controller = { device_id: 'browser', device_name: 'Browser', online: true };

const agentContract = await loadSource('../../shared/agent-harness/contract.generated.ts');
const navigationModule = await loadSource('../src/services/MobileNavigationStore.ts', {
  '../../../shared/agent-harness/contract.generated': agentContract.url,
});
const { loadMobileNavigation, saveMobileNavigation, clearMobileNavigation } = await import(navigationModule.url);

test('same-tab reload restores the selected device and session only within the authenticated QR scope', () => {
  const entries = new Map();
  const storage = {
    getItem: key => entries.get(key) ?? null,
    setItem: (key, value) => entries.set(key, value),
    removeItem: key => entries.delete(key),
  };
  const scope = {
    accountId: 'account-a', controllerDeviceId: 'browser-a',
    relayUrl: 'https://relay.example.com', routeKey: '/relay/r/room/#/pair?did=desktop-a',
  };
  const navigation = { deviceId: 'desktop-b', session: { id: 'session-b', name: 'Task B', agentType: 'agentic' } };
  saveMobileNavigation(scope, navigation, storage);
  assert.deepEqual(loadMobileNavigation(scope, storage), {
    ...navigation, session: { ...navigation.session, agentType: 'Standard' },
  });
  for (const replacement of [
    { accountId: 'account-b' }, { controllerDeviceId: 'browser-b' },
    { relayUrl: 'https://another-relay.example.com' }, { routeKey: '/relay/r/new/#/pair?did=desktop-c' },
  ]) {
    assert.equal(loadMobileNavigation({ ...scope, ...replacement }, storage), null);
  }
  clearMobileNavigation(storage);
  assert.equal(loadMobileNavigation(scope, storage), null);
});

test('unsupported or unreadable navigation stays intact and unavailable browser storage is optional', () => {
  for (const raw of ['{', JSON.stringify({ version: 2, deviceId: 'future' })]) {
    let retained = raw;
    const storage = { getItem: () => retained, setItem: value => { retained = value; }, removeItem: () => { retained = ''; } };
    assert.equal(loadMobileNavigation({}, storage), null);
    assert.equal(retained, raw);
  }
  const blocked = { getItem: () => { throw new Error('blocked'); }, setItem: () => { throw new Error('blocked'); }, removeItem: () => { throw new Error('blocked'); } };
  assert.equal(loadMobileNavigation({}, blocked), null);
  assert.doesNotThrow(() => saveMobileNavigation({}, { deviceId: 'device' }, blocked));
  assert.doesNotThrow(() => clearMobileNavigation(blocked));
});

test('empty, offline and controller-only directories leave the account without a target', () => {
  for (const devices of [[], [offline], [controller], [offline, controller]]) {
    assert.equal(selectAccountDevice(devices, 'browser'), null);
  }
});

test('online selection preserves exact QR targeting and supports later device availability', () => {
  assert.equal(selectAccountDevice([controller, offline, online], 'browser'), online);
  assert.equal(selectAccountDevice([online], 'browser', offline.device_id), null);
  assert.equal(selectAccountDevice([offline, online], 'browser', offline.device_id), null);
  const reconnected = { ...offline, online: true };
  assert.equal(selectAccountDevice([reconnected, online], 'browser', offline.device_id), reconnected);
});

/**
 * Loads the transpiled CloudAccountClient with a scripted `fetch`; the caller
 * gets the client module plus every recorded request. `fetch` and `window`
 * are restored when the returned promise settles.
 */
async function withLocalClient(handler) {
  const encryption = await loadSource('../src/services/E2EEncryption.ts');
  const authModule = await loadSource('../src/services/CloudAccountClient.ts', {
    './E2EEncryption': encryption.url, './pairingLink': links.url, ...clientBuildImports,
  });
  const loaded = await import(authModule.url);
  const originalFetch = globalThis.fetch;
  const originalWindow = globalThis.window;
  const requests = [];
  globalThis.window = { setTimeout, clearTimeout };
  globalThis.fetch = async (url, options) => {
    const body = options?.body ? JSON.parse(options.body) : null;
    requests.push({ url: String(url), body });
    return handler(String(url), body);
  };
  const restore = () => {
    globalThis.fetch = originalFetch;
    if (originalWindow === undefined) delete globalThis.window;
    else globalThis.window = originalWindow;
  };
  const client = new loaded.CloudAccountClient('https://obf.test.linkerpi.com');
  return {
    client, requests, restore,
    CloudAccountRequestError: loaded.CloudAccountRequestError,
    CloudAccountTransportError: loaded.CloudAccountTransportError,
    normalizeAccountPhone: loaded.normalizeAccountPhone,
  };
}

const localPairBody = {
  accessToken: 'local-access-1',
  accessExpiresAt: 1893456000,
  refreshToken: 'local-refresh-1',
  refreshExpiresAt: 1896048000,
  user: { userId: 'local-abc123', login: 'user-abc123', phoneMasked: '138****0002', hasPassword: false },
};

test('phone sign-in sends the code, exchanges it locally, then registers the stored device key', async () => {
  const { x25519 } = await import('@noble/curves/ed25519.js');
  const browserPrivateKey = new Uint8Array(32).fill(7);
  const exchanges = [];
  const { client, requests, normalizeAccountPhone, restore } = await withLocalClient((url, body) => {
    if (url.endsWith('/api/auth/local/sms/send')) {
      assert.deepEqual(body, { phone: '13800000002' });
      return Response.json({ status: 'sent' }, { status: 202 });
    }
    if (url.endsWith('/api/auth/local/login')) {
      assert.deepEqual(body, { phone: '13800000002', code: '123456' });
      assert.equal('password' in body, false, 'code sign-in must not also send a password');
      return Response.json(localPairBody);
    }
    if (url.endsWith('/api/auth/login')) {
      exchanges.push(body);
      return Response.json({ token: 'device-token-1', user_id: 'local-abc123' });
    }
    throw new Error(`Unexpected request ${url}`);
  });
  try {
    // Form-local normalization mirrors the Relay, so the phone crosses the wire normalized.
    assert.equal(normalizeAccountPhone('+86 138-0000-0002'), '13800000002');
    await client.smsSend('13800000002');
    const pair = await client.localLogin('13800000002', { code: '123456' });
    assert.equal(pair.refreshToken, 'local-refresh-1');
    assert.equal(pair.user.phoneMasked, '138****0002');
    assert.equal(pair.user.hasPassword, false);
    const first = await client.login(pair.accessToken, 'browser', browserPrivateKey);
    const second = await client.login(pair.accessToken, 'browser', browserPrivateKey);
    assert.equal(first.userId, 'local-abc123');
    assert.deepEqual(first.masterKey, second.masterKey);
    for (const body of exchanges) {
      assert.equal(body.access_token, 'local-access-1');
      assert.equal(body.device_id, 'browser');
      assert.equal(body.password_hash, undefined);
      assert.equal(body.clientProtocol, CLIENT_PROTOCOL_VERSION);
      assert.equal(body.clientVersion, CLIENT_VERSION);
      assert.equal(Buffer.from(body.public_key, 'base64').length, 32);
    }
    assert.equal(exchanges[0].public_key, exchanges[1].public_key, 'the stored X25519 key is never regenerated');
    assert.deepEqual(Buffer.from(exchanges[0].public_key, 'base64'), Buffer.from(x25519.getPublicKey(first.masterKey)));
    assert.equal(requests[0].url.endsWith('/api/auth/local/sms/send'), true);
  } finally { restore(); }
});

test('password sign-in sends exactly one credential and surfaces relay refusals', async () => {
  const { client, requests, CloudAccountRequestError, restore } = await withLocalClient(url => {
    if (url.endsWith('/api/auth/local/login')) {
      return Response.json({ error: 'invalid code or password' }, { status: 401 });
    }
    throw new Error(`Unexpected request ${url}`);
  });
  const attempt = client.localLogin('13800000002', { password: 'fixture-password' });
  assert.deepEqual(requests[0].body, { phone: '13800000002', password: 'fixture-password' });
  assert.equal('code' in requests[0].body, false, 'password sign-in must not also send a code');
  await assert.rejects(attempt, error => error instanceof CloudAccountRequestError
    && error.status === 401 && error.message === 'invalid code or password' && error.retryAfterSeconds === null);
  restore();
});

test('an SMS cooldown answer carries the relay retry window', async () => {
  const { client, CloudAccountRequestError, restore } = await withLocalClient(url => {
    if (url.endsWith('/api/auth/local/sms/send')) {
      return Response.json(
        { error: 'please wait before requesting another code', retry_after_secs: 42 },
        { status: 429 },
      );
    }
    throw new Error(`Unexpected request ${url}`);
  });
  await assert.rejects(client.smsSend('13800000002'), error => error instanceof CloudAccountRequestError
    && error.status === 429 && error.retryAfterSeconds === 42
    && error.message === 'please wait before requesting another code');
  restore();
});

test('the local form follows the relay capability contract', async () => {
  const { client, CloudAccountTransportError, restore } = await withLocalClient(() => {
    throw new Error('Unexpected request');
  });
  try {
    let answer = { capabilities: ['device_alias_v1'] };
    globalThis.fetch = async () => ({ ok: true, status: 200, json: async () => answer });
    assert.equal(await client.supportsLocalAuth(), false, 'a relay without local_auth_v1 is an old relay');
    answer = { capabilities: ['device_alias_v1', 'local_auth_v1'] };
    assert.equal(await client.supportsLocalAuth(), true);
    globalThis.fetch = async () => ({ ok: false, status: 404, json: async () => ({}) });
    assert.equal(await client.supportsLocalAuth(), false, 'a relay without /api/info is an old relay');
    globalThis.fetch = async () => { throw new TypeError('Failed to fetch'); };
    await assert.rejects(client.supportsLocalAuth(), error => error instanceof CloudAccountTransportError
      && error.message === 'Could not reach the account service.');
  } finally { restore(); }
});

test('local answers must be complete and refresh replays are definitive', async () => {
  const { client, requests, CloudAccountRequestError, CloudAccountTransportError, restore } = await withLocalClient(url => {
    if (url.endsWith('/api/auth/local/login')) return Response.json({ accessToken: 'local-access-1' });
    if (url.endsWith('/api/auth/local/refresh')) return Response.json({ error: 'refresh token expired' }, { status: 401 });
    throw new Error(`Unexpected request ${url}`);
  });
  try {
    await assert.rejects(client.localLogin('13800000002', { code: '123456' }),
      error => error instanceof CloudAccountTransportError && /incomplete/.test(error.message));
    await assert.rejects(client.localRefresh('local-refresh-1'), error => error instanceof CloudAccountRequestError
      && error.status === 401);
    assert.deepEqual(requests[1].body, { refreshToken: 'local-refresh-1' });
  } finally { restore(); }

  const rotation = await withLocalClient(url => {
    if (url.endsWith('/api/auth/local/refresh')) {
      return Response.json({ ...localPairBody, accessToken: 'local-access-2', refreshToken: 'local-refresh-2' });
    }
    throw new Error(`Unexpected request ${url}`);
  });
  try {
    const rotated = await rotation.client.localRefresh('local-refresh-1');
    assert.equal(rotated.accessToken, 'local-access-2');
    assert.equal(rotated.refreshToken, 'local-refresh-2');
  } finally { rotation.restore(); }
});

test('phone normalization mirrors the relay contract', async () => {
  const encryption = await loadSource('../src/services/E2EEncryption.ts');
  const authModule = await loadSource('../src/services/CloudAccountClient.ts', {
    './E2EEncryption': encryption.url, './pairingLink': links.url, ...clientBuildImports,
  });
  const { normalizeAccountPhone } = await import(authModule.url);
  for (const [raw, expected] of [
    ['13800000002', '13800000002'],
    ['+8613800000002', '13800000002'],
    ['8613800000002', '13800000002'],
    ['+86 138-0000-0002', '13800000002'],
    [' 13800000002 ', '13800000002'],
  ]) assert.equal(normalizeAccountPhone(raw), expected);
  for (const invalid of [
    '1380000002', '23800000002', '12800000002', '1380000000a', 'abc', '', '+86138000000021', '0861380000000',
  ]) assert.equal(normalizeAccountPhone(invalid), null, `${invalid} must not normalize`);
});

test('psk pairing derivation and code mirror the Rust contract', async () => {
  const encryption = await loadSource('../src/services/E2EEncryption.ts');
  const pairingCodec = await loadSource('../src/services/PskPairing.ts');
  const { deriveDeviceMessageKey, deriveDeviceMessageKeyPsk, pskKeyIdOf } = await import(encryption.url);
  const { encodePskPairingCode, parsePskPairingCode } = await import(pairingCodec.url);
  const { x25519 } = await import('@noble/curves/ed25519.js');
  const alice = new Uint8Array(32).fill(7);
  const bob = new Uint8Array(32).fill(11);
  const psk = new Uint8Array(32).fill(91);
  const alicePub = x25519.getPublicKey(alice);
  const bobPub = x25519.getPublicKey(bob);
  const sending = deriveDeviceMessageKeyPsk(alice, bobPub, psk);
  const receiving = deriveDeviceMessageKeyPsk(bob, alicePub, psk);
  // Pinned vector shared with `remote_connect/device_crypto.rs`.
  assert.equal(Buffer.from(sending).toString('hex'), 'f55f939ca726bfae284fb0263acbadbd8b010f4ff5f7d47663f0a0706149c827');
  assert.deepEqual(sending, receiving);
  assert.notDeepEqual(sending, deriveDeviceMessageKey(alice, bobPub));
  assert.notDeepEqual(sending, deriveDeviceMessageKeyPsk(alice, bobPub, new Uint8Array(32).fill(92)));
  assert.equal(pskKeyIdOf(psk), 'b95439ddd3dec598');

  const payload = {
    psk,
    offererDeviceId: '0123456789abcdef0123456789abcdef',
    offererPublicKey: alicePub,
  };
  const code = encodePskPairingCode(payload);
  // Pinned vector shared with `remote_connect/pairing.rs`.
  assert.equal(code, '05DN-PPTV-BDDN-PPTV-BDDN-PPTV-BDDN-PPTV-BDDN-PPTV-BDDN-PPTV-BDDN-P093-8NKR-KAYD-XW0J-6HB7-H6NW-VVRK-QS7Y-NTQJ-0K3Z-TCTR-ZJE0-0WGR-G78Q-89W1-50H7-XHKM-YDZQ-ZTBV-DQDQ-TCAQ');
  const parsed = parsePskPairingCode(code);
  assert.equal(parsed.offererDeviceId, payload.offererDeviceId);
  assert.deepEqual(parsed.psk, psk);
  assert.deepEqual(parsed.offererPublicKey, alicePub);
  const noisy = code.toLowerCase().replace(/-/g, ' ');
  assert.equal(parsePskPairingCode(noisy).offererDeviceId, payload.offererDeviceId);
  const flipped = code.replace('A', 'B');
  assert.throws(() => parsePskPairingCode(flipped), /check failed|character/i);
  assert.throws(() => parsePskPairingCode(code.slice(0, -8)), /length/i);
});

test('account UI entry precedes discovery and mounts no remote workspace surface', async () => {
  const pairing = await readFile(new URL('../src/pages/PairingPage.tsx', import.meta.url), 'utf8');
  const direct = pairing.slice(pairing.indexOf('const connect ='), pairing.indexOf('  useEffect('));
  assert.doesNotMatch(direct, /saveSession|saveCloudAccountSession/, 'restoring account state must not republish a new login');
  assert.match(pairing, /accountStore\.saveSession\(browser, candidate, isCurrent, pair\.refreshToken\)/);
  assert.match(direct, /store\.setControlTarget\(null\)/);
  assert.match(direct, /onPairedRef\.current/);
  assert.doesNotMatch(direct, /listDevices\(|sendDeviceRpc\(|\.online|throw new Error/);
  const app = await readFile(new URL('../src/App.tsx', import.meta.url), 'utf8');
  assert.match(app, /useConnectionHealth\(accountDirectoryOpen \? null : sessionMgr\)/);
  assert.match(app, /!accountDirectoryOpen && page !== 'pairing' && sessionMgrRef\.current/);
  assert.equal((app.match(/\{renderDetailPage\(\)\}/g) || []).length, 1, 'one gated detail tree must preserve chat state across layout changes');
  const devices = await readFile(new URL('../src/pages/DevicesPage.tsx', import.meta.url), 'utf8');
  assert.match(devices, /if \(!d\.online \|\| !isDeviceControllable\(d\) \|\| switchingId\) return/);
  assert.match(devices, /automaticSelectionAttemptedRef\.current = true/);
  assert.match(devices, /selectDevice\(target, false\)/, 'initial account selection must not require a new peer command');
  assert.ok(devices.indexOf('await client.sendDeviceRpc') < devices.indexOf('client.setTargetDeviceId(d.device_id)'));
});

test('sign-in is an inline phone form gated by the relay capability probe', async () => {
  const page = await readFile(new URL('../src/pages/PairingPage.tsx', import.meta.url), 'utf8');
  const form = await readFile(new URL('../src/components/PairingForm.tsx', import.meta.url), 'utf8');
  const client = await readFile(new URL('../src/services/CloudAccountClient.ts', import.meta.url), 'utf8');
  // The GitHub/email popup flow and its trust anchors are gone for good.
  for (const source of [page, form, client]) {
    assert.doesNotMatch(source, /window\.open|about:blank|auth\.openbitfun\.com|github\.com|\/api\/auth\/github\//);
  }
  assert.match(page, /supportsLocalAuth\(\)/, 'the relay contract is probed before the form goes live');
  assert.match(form, /localAuthUnsupported/, 'an old relay shows an explicit unsupported state');
  assert.match(form, /resendCodeIn/);
  assert.match(form, /usePasswordLogin/);
  assert.match(form, /useCodeLogin/);
  // A refused rotation keeps the expired-session semantics: sign in again.
  const app = await readFile(new URL('../src/App.tsx', import.meta.url), 'utf8');
  assert.match(app, /recoverExpiredAccountSession/);
  assert.match(app, /clearSession\(token, 'expired'\)/);
});

test('alias editing replaces the device row instead of repeating its name', async () => {
  const devices = await readFile(new URL('../src/pages/DevicesPage.tsx', import.meta.url), 'utf8');
  const start = devices.indexOf('{editingId === d.device_id ? (');
  assert.ok(start > 0, 'the alias editor must be a mutually exclusive branch');
  const elseAt = devices.indexOf(') : (', start);
  const rowAt = devices.indexOf('<MobileListRow', start);
  assert.ok(elseAt > start, 'the editing branch needs an else branch for the row');
  assert.ok(rowAt > elseAt, 'the device row must render only while the alias editor is closed');
  const editingBranch = devices.slice(start, elseAt);
  assert.match(editingBranch, /devices-page__alias-editor/);
  assert.match(editingBranch, /devices\.saveAlias/);
});

test('official and local invitations share strict device-only targeting', async () => {
  const { currentRelayUrl, pairingRelayUrl, accountDeviceIdFromHash } = await import(links.url);
  for (const base of ['https://obf.test.linkerpi.com/', 'http://192.168.1.9:9700/']) {
    const url = new URL(`${base}#/pair?did=desktop-1`);
    assert.equal(currentRelayUrl(url), base.replace(/\/$/, ''));
    assert.equal(accountDeviceIdFromHash(url.hash), 'desktop-1');
    for (const hash of ['did=a&did=b', 'did=a&pk=untrusted', 'did=a&relay=https://evil.example',
      'did=%2Fother', 'room=room&pk=key', 'did=..']) {
      assert.equal(accountDeviceIdFromHash(`#/pair?${hash}`), null);
    }
  }
  for (const base of ['https://evil.example/', 'https://obf.test.linkerpi.com.evil.example/',
    'https://user@obf.test.linkerpi.com/', 'http://obf.test.linkerpi.com/',
    'https://obf.test.linkerpi.com/relay/']) {
    assert.equal(pairingRelayUrl(base), null);
  }
  assert.equal(accountDeviceIdFromHash('#/pair?did=desktop'), 'desktop');
  assert.equal(accountDeviceIdFromHash('#/chat?did=desktop'), null);
});

const githubModule = await loadSource('../src/services/GitHubAccountProfile.ts');
const { loadGitHubAccountProfile, normalizeGitHubProfile } = await import(githubModule.url);
const githubUser = { id: 123, login: 'example', avatar_url: 'https://avatars.githubusercontent.com/u/123' };

test('legacy numeric GitHub profiles keep their public metadata contract', async () => {
  assert.throws(() => normalizeGitHubProfile('124', githubUser));
  assert.equal(normalizeGitHubProfile('123', { ...githubUser, avatar_url: 'https://other.example/avatar' }).avatarUrl, '');
  const profiles = [];
  await loadGitHubAccountProfile('123', p => profiles.push(p), new AbortController().signal, null, async (url, options) => {
    assert.equal(url, 'https://api.github.com/user/123');
    assert.equal(options.credentials, 'omit');
    assert.deepEqual(options.headers, { Accept: 'application/vnd.github+json' });
    return { ok: true, json: async () => githubUser };
  });
  assert.equal(profiles[0].login, 'example');
});

test('relay-local account ids degrade to the letter avatar without any request', async () => {
  // Local accounts are `local-<hex>`; only numeric GitHub ids resolve avatars.
  for (const userId of ['local-0123456789abcdef', 'local-']) {
    const applied = [];
    await loadGitHubAccountProfile(userId, p => applied.push(p), new AbortController().signal, null,
      async () => { throw new Error('relay-local ids must never reach the GitHub profile service'); });
    assert.deepEqual(applied, []);
  }
});

test('GitHub cached profiles survive refresh failure and fresh cache avoids a request', async () => {
  for (const age of [0, 2 * 86400000]) {
    const profile = { ...normalizeGitHubProfile('123', githubUser), fetchedAt: Date.now() - age };
    let requests = 0;
    const applied = [];
    await loadGitHubAccountProfile('123', p => applied.push(p), new AbortController().signal,
      { getItem: () => JSON.stringify(profile), setItem: () => assert.fail('must retain cache') },
      async () => { requests++; throw new Error('offline'); });
    assert.equal(applied[0].login, 'example');
    assert.equal(requests, age ? 1 : 0);
  }
});

test('GitHub profile ignores corrupt/foreign cache and stale account responses', async () => {
  for (const raw of ['{', JSON.stringify({ userId: '456', fetchedAt: Date.now(), login: 'another' })]) {
    const controller = new AbortController();
    await loadGitHubAccountProfile('123', () => assert.fail('stale profile applied'), controller.signal,
      { getItem: () => raw, setItem: () => assert.fail('stale profile cached') },
      async () => {
        controller.abort();
        return { ok: true, json: async () => githubUser };
      });
  }
});


function memoryStorage() {
  const entries = new Map();
  return {
    getItem: key => entries.get(key) ?? null,
    setItem: (key, value) => entries.set(key, value),
    removeItem: key => entries.delete(key),
  };
}

const encryptionModule = await loadSource('../src/services/E2EEncryption.ts');
const accountStoreModule = await loadSource('../src/services/CloudAccountSessionStore.ts', {
  './E2EEncryption': encryptionModule.url, './pairingLink': links.url,
});
const accountStore = await import(accountStoreModule.url);
const storedAccount = {
  relayUrl: 'http://192.168.1.9:9700', username: '123', controllerDeviceId: 'browser-a',
  session: { token: 'test-token', userId: '123', masterKey: new Uint8Array(32).fill(7) },
};

test('legacy v2 account proof remains readable and scoped for migration', () => {
  const storage = memoryStorage();
  const legacy = JSON.stringify({
    version: 2, relay_url: storedAccount.relayUrl, username: '123', token: 'test-token',
    user_id: '123', device_secret: Buffer.alloc(32, 7).toString('base64'), controller_device_id: 'browser-a',
  });
  storage.setItem('openbitfun.mobile.account_session.v2', legacy);
  const restored = accountStore.loadMatchingCloudAccountSession(storedAccount.relayUrl, '', 'browser-a', storage);
  assert.deepEqual(restored, storedAccount);
  accountStore.saveCloudAccountSession(restored, storage);
  assert.deepEqual(JSON.parse(storage.getItem('openbitfun.mobile.account_session.v2')), JSON.parse(legacy));
  for (const [relay, username, controllerId] of [
    ['https://obf.test.linkerpi.com', '', 'browser-a'],
    [storedAccount.relayUrl, '', 'browser-b'], [storedAccount.relayUrl, '456', 'browser-a'],
  ]) assert.equal(accountStore.loadMatchingCloudAccountSession(relay, username, controllerId, storage), null);
  assert.equal(storage.getItem('openbitfun.mobile.account_session.v2'), legacy);
  accountStore.clearCloudAccountSession(storage);
  assert.equal(accountStore.loadMatchingCloudAccountSession(storedAccount.relayUrl, '', 'browser-a', storage), null);
  for (const raw of ['{', JSON.stringify({ version: 99 })]) {
    storage.setItem('openbitfun.mobile.account_session.v2', raw);
    assert.equal(accountStore.loadMatchingCloudAccountSession(storedAccount.relayUrl, '', 'browser-a', storage), null);
    assert.equal(storage.getItem('openbitfun.mobile.account_session.v2'), raw);
  }
});


test('presence uses the authenticated browser ID across managers, tabs and reloads', async () => {
  const module = await loadSource('../src/services/controlClientIdentity.ts');
  const { getControlClientIdentity } = await import(module.url);
  const identity = getControlClientIdentity('browser-device-a');
  assert.equal(identity.id, 'browser-device-a');
  assert.ok(identity.name);
  const { getControlClientIdentity: reloaded } = await import(`${module.url}#reloaded`);
  assert.deepEqual(reloaded('browser-device-a'), identity);
  assert.notEqual(reloaded('browser-device-b').id, identity.id);
});

test('tab navigation preserves explicit disconnect and migrates only a matching legacy controller', async () => {
  const { migrateMobileNavigationController } = await import(navigationModule.url);
  const storage = memoryStorage();
  const scope = { accountId: 'account-a', controllerDeviceId: 'tab-a', relayUrl: storedAccount.relayUrl, routeKey: '/' };
  saveMobileNavigation(scope, { deviceId: 'desktop-a', disconnected: true }, storage);
  migrateMobileNavigationController('other-account', scope.relayUrl, 'tab-a', 'browser-a', storage);
  assert.equal(loadMobileNavigation({ ...scope, controllerDeviceId: 'browser-a' }, storage), null);
  migrateMobileNavigationController(scope.accountId, scope.relayUrl, 'tab-a', 'browser-a', storage);
  assert.deepEqual(loadMobileNavigation({ ...scope, controllerDeviceId: 'browser-a' }, storage), {
    deviceId: 'desktop-a', disconnected: true, session: undefined,
  });
});
