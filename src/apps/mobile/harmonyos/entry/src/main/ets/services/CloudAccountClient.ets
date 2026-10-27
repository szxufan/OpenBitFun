import { HostSessionStream, HostSessionCallbacks, HostSessionSource, HostStreamHint, HostStreamPage, HostStreamReadRequest, HostStreamHintPlaintext, parseHostStreamHint } from './HostSessionStream';
import { AccountRealtime } from './AccountRealtime';
import { CLIENT_PROTOCOL_VERSION, clientVersion } from './ClientBuild';
import { normalizeAccountRelayUrl, normalizeManualRelayUrl, OFFICIAL_RELAY_URL } from './AccountDeviceLink';
import { http } from '@kit.NetworkKit';
import { CloudAccountCrypto } from './CloudAccountCrypto';
import { X25519 } from './X25519';
import { Encoding } from './Encoding';
import { HarmonyRemoteCryptoCipher } from './RemoteCrypto';
import { OhosError } from './OhosError';
import { RemoteLogger } from './RemoteLogger';
import { RemoteI18n } from '../i18n/RemoteI18n';
import { CommandStatusResponse, EncryptedPayload, RemoteCommand } from '../model/RemoteModels';

interface AccountAuthResponse { token: string; user_id: string; }
interface LoginRequest {
  access_token: string; device_id: string; device_name: string; device_kind: string;
  public_key: string; request_id: string;
  /** See ClientBuild.ets; the Relay stores both and gates on the protocol. */
  clientVersion: string; clientProtocol: number;
}
interface PeerPublicKey { public_key: string; }
/** Relay error body: `{"error": string, "retry_after_secs"?: number}` — snake_case, unlike the token payloads. */
interface RelayErrorResponse { error?: string; retry_after_secs?: number; }
/** `ephemeral` frame the relay forwards for a host's `DeviceEvent`; the body is ciphertext. */
interface DeviceEventFrame { type?: string; sourceDeviceId?: string; params?: EncryptedPayload; }

/** `GET /api/info`; only the capability list is read here. */
interface RelayInfoWire { capabilities?: string[]; }
interface SmsSendWireRequest { phone: string; }
interface SmsSendWireResponse { status?: string; }
/** Login body stays snake_case on the wire (`phone`/`code`/`password` are single-word anyway). */
interface LocalLoginWireRequest { phone: string; code?: string; password?: string; }
/** Refresh body is camelCase, matching the relay's `LocalRefreshRequest`. */
interface LocalRefreshWireRequest { refreshToken: string; }
interface LocalUserWire { userId?: string; login?: string; phoneMasked?: string; hasPassword?: boolean; }
interface LocalTokenPairWire {
  accessToken?: string; accessExpiresAt?: number;
  refreshToken?: string; refreshExpiresAt?: number; user?: LocalUserWire;
}

/** Relay-local account identity as the relay reports it with a token pair. */
export interface LocalAccountUser {
  userId: string;
  login: string;
  phoneMasked: string;
  hasPassword: boolean;
}

/** The relay-local token pair returned by sign-in and by every refresh. */
export interface LocalAccountTokens {
  accessToken: string;
  accessExpiresAt: number;
  refreshToken: string;
  refreshExpiresAt: number;
  user: LocalAccountUser;
}

/**
 * An inline sign-in credential from the login form. Exactly one of
 * `code`/`password` must carry a value; `localLogin` enforces that on the wire.
 */
export interface LocalAccountCredential {
  relayUrl: string;
  phone: string;
  code?: string;
  password?: string;
}

interface NetworkRequestFailure { code?: number; }

export class CloudAccountTransportError extends Error {
  readonly code: number;
  constructor(code: number) {
    super(`Account transport failed (code=${code}).`);
    this.code = code;
  }
}

export class CloudAccountRequestError extends Error {
  readonly statusCode: number;
  /** Cooldown the relay attached to a 429, when it sent one. */
  readonly retryAfterSecs?: number;

  constructor(message: string, statusCode: number, retryAfterSecs?: number) {
    super(message);
    this.name = 'CloudAccountRequestError';
    this.statusCode = statusCode;
    this.retryAfterSecs = retryAfterSecs;
  }
}

/**
 * The relay does not advertise `local_auth_v1`, so the phone-number flows are
 * not offered against it. The message is localized where it is thrown.
 */
export class RelayLocalAuthUnsupportedError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'RelayLocalAuthUnsupportedError';
  }
}

export interface CloudAccountSession {
  token: string;
  userId: string;
  masterKey: Uint8Array;
}

/** The live account client plus the session it is currently signed in with. */
export interface CloudAccountBinding {
  client: CloudAccountClient;
  relayUrl: string;
  session: CloudAccountSession;
}

export interface CloudAccountDevice {
  deviceId: string;
  deviceName: string;
  deviceKind?: string;
  online: boolean;
  lastSeenAt?: number;
  /**
   * Relay-computed mutual-control compatibility of this device with this one.
   *
   * `false` means confirmed incompatible: either a client build/protocol
   * mismatch or a peer that reported no version information (an older client).
   * Absent only on an older Relay that does not gate at all, which must be
   * treated as "unknown but usable", never as incompatible.
   */
  compatible?: boolean;
}

/** The single gate every control entry point must reuse; see `compatible`. */
export function isDeviceControllable(device: CloudAccountDevice): boolean {
  return device.compatible !== false;
}

/** A device credential minted for another device, never for this one. */
export interface CloudProvisionedDevice {
  token: string;
  userId: string;
  deviceId: string;
  deviceSecret: Uint8Array;
}

interface ProvisionDeviceRequest {
  public_key: string;
  device_id: string;
  device_name: string;
  device_kind: string;
  request_id: string;
}

interface ProvisionDeviceWire {
  token: string;
  user_id: string;
  device_id: string;
}

interface CloudAccountDeviceWire {
  device_id: string;
  device_name: string;
  device_kind?: string;
  online: boolean;
  last_seen_at?: number;
  compatible?: boolean;
}

/** Default relay address for the account flow; the sign-in form makes it editable. */
export const DEFAULT_CLOUD_RELAY_URL: string = OFFICIAL_RELAY_URL;

/** Capability a relay advertises when `/api/auth/local/*` is live; see the relay's `routes/api.rs`. */
const LOCAL_AUTH_CAPABILITY: string = 'local_auth_v1';

/** Device kinds the relay accepts; mirrors `relay-service/src/db.rs::DEVICE_KINDS`. */
const DEVICE_KIND_DESKTOP: string = 'desktop';
const DEVICE_KIND_CLI: string = 'cli';
const DEVICE_KIND_MOBILE: string = 'mobile';
const DEVICE_KIND_WATCH: string = 'watch';

/**
 * Device names our own non-desktop builds register under, lowercased.
 *
 * Only labels we hardcode ourselves belong here: the Android client reports
 * `Build.MODEL` (`android/.../DeviceInstall.kt:27`), an arbitrary string that
 * guessing at would hide desktops as readily as phones.
 *
 * Kept in step with `shared/core-transport/.../CloudAccountClient.kt`, which
 * carries the same list for the Android and iOS clients.
 */
const KNOWN_NON_DESKTOP_DEVICE_NAMES: string[] = ['harmonyos phone', 'harmonyos watch'];

/**
 * Whether a relay device row is a host, and so controllable from a phone.
 *
 * A row that reports its kind is taken at its word: a desktop and a CLI host run
 * the same control plane, so both are targets, while a phone or a watch is only
 * ever a controller. A row without one predates the relay learning about kinds,
 * and is judged by two weaker signals: this phone's own row is never a host, and
 * neither is one carrying a name our own builds register under. Anything else
 * stays visible — hiding a real host would strand the user, while a stale phone
 * row disappears the next time that phone logs in against a relay that stores
 * kinds.
 */
function isHostDeviceRow(device: CloudAccountDeviceWire, selfDeviceId: string): boolean {
  const kind = (device.device_kind || '').trim();
  if (kind.length > 0) {
    return kind === DEVICE_KIND_DESKTOP || kind === DEVICE_KIND_CLI;
  }
  if (selfDeviceId.length > 0 && device.device_id === selfDeviceId) {
    return false;
  }
  const name = (device.device_name || '').trim().toLowerCase();
  return KNOWN_NON_DESKTOP_DEVICE_NAMES.indexOf(name) < 0;
}

/** Client for the current desktop relay account protocol. */
interface DeviceKeyPair { privateKey: Uint8Array; publicKey: Uint8Array; }

/**
 * Canonical request base for a relay address. Scanned links and the official
 * constant resolve through the strict allow-list; an address the user typed
 * into the sign-in form is accepted from any origin (see
 * `normalizeManualRelayUrl`).
 */
function accountRelayBase(relayUrl: string): string {
  return normalizeAccountRelayUrl(relayUrl) || normalizeManualRelayUrl(relayUrl);
}

/** Validates a relay-local token-pair response; the relay owns the wire shape. */
function toLocalTokens(wire: LocalTokenPairWire): LocalAccountTokens {
  const user = wire.user;
  if (!wire.accessToken || !wire.refreshToken || !user || !user.userId) {
    throw new Error('Relay returned an incomplete local account response.');
  }
  return {
    accessToken: wire.accessToken,
    accessExpiresAt: typeof wire.accessExpiresAt === 'number' ? wire.accessExpiresAt : 0,
    refreshToken: wire.refreshToken,
    refreshExpiresAt: typeof wire.refreshExpiresAt === 'number' ? wire.refreshExpiresAt : 0,
    user: {
      userId: user.userId,
      login: user.login || '',
      phoneMasked: user.phoneMasked || '',
      hasPassword: user.hasPassword === true
    }
  };
}

export class CloudAccountClient {
  private realtime?: AccountRealtime;
  private realtimeUrl: string = '';
  private realtimeToken: string = '';

  private connection(relayUrl: string, token: string): AccountRealtime {
    const url = accountRelayBase(relayUrl);
    if (!url) throw new Error('Invalid Relay URL');
    if (this.realtime && this.realtimeUrl === url && this.realtimeToken === token) return this.realtime;
    this.realtime?.close();
    this.realtimeUrl = url; this.realtimeToken = token;
    this.realtime = new AccountRealtime(url, token);
    return this.realtime;
  }

  closeAccount(): void {
    this.realtime?.close(); this.realtime = undefined;
    this.realtimeUrl = ''; this.realtimeToken = '';
    this.peerKeys.clear();
  }

  /**
   * Pairwise message keys by target device, derived from the peer's published
   * X25519 key. Stream hints arrive unsolicited, so their key must be at hand;
   * a stale entry is dropped when a frame fails to decrypt.
   */
  private readonly peerKeys: Map<string, Promise<Uint8Array>> = new Map();

  private peerMessageKey(relayUrl: string, session: CloudAccountSession, target: string): Promise<Uint8Array> {
    const cacheKey = `${session.userId}\n${target}`;
    const cached = this.peerKeys.get(cacheKey);
    if (cached) return cached;
    const derived = this.request<PeerPublicKey>(relayUrl,
      `/api/devices/${encodeURIComponent(target)}/key`, 'GET', undefined, session.token)
      .then((peer: PeerPublicKey) => CloudAccountCrypto.deriveDeviceKey(session.masterKey, Encoding.base64ToBytes(peer.public_key)));
    this.peerKeys.set(cacheKey, derived);
    derived.catch((): void => { this.peerKeys.delete(cacheKey); });
    return derived;
  }

  private forgetPeerKey(session: CloudAccountSession, target: string): void {
    this.peerKeys.delete(`${session.userId}\n${target}`);
  }

  private readonly cipher: HarmonyRemoteCryptoCipher = new HarmonyRemoteCryptoCipher();

  /**
   * Whether [relayUrl] advertises relay-local accounts (`local_auth_v1`).
   *
   * The login form probes before offering phone-number sign-in: an older relay
   * 404s every `/api/auth/local/*` route, which must surface as an explicit
   * unsupported state instead of a form whose every request fails.
   */
  async relaySupportsLocalAuth(relayUrl: string): Promise<boolean> {
    const info = await this.request<RelayInfoWire>(relayUrl, '/api/info', 'GET', undefined, '', 15000);
    return (info.capabilities || []).indexOf(LOCAL_AUTH_CAPABILITY) >= 0;
  }

  /** Probes [relayUrl] and throws when the relay lacks the local-account capability. */
  async requireLocalAuth(relayUrl: string): Promise<void> {
    if (!await this.relaySupportsLocalAuth(relayUrl)) {
      throw new RelayLocalAuthUnsupportedError(RemoteI18n.t('account.loginServerUnsupported'));
    }
  }

  /**
   * `POST /api/auth/local/sms/send` — deliver a verification code to a phone.
   *
   * A 202 means the relay accepted the request; it deliberately never reveals
   * whether the phone already owns an account. A 429 carries
   * `retry_after_secs`, exposed on the thrown error so the resend countdown
   * can honor the relay's own cooldown.
   */
  async smsSend(relayUrl: string, phone: string): Promise<void> {
    const body: SmsSendWireRequest = { phone };
    await this.post<SmsSendWireResponse>(relayUrl, '/api/auth/local/sms/send', body);
  }

  /**
   * `POST /api/auth/local/login` with exactly one of verification [code] or
   * [password]. First use of a phone registers the account unless the relay
   * closed registration; that refusal (403) and every other relay-authored
   * error text travel verbatim in `CloudAccountRequestError.message`.
   */
  async localLogin(relayUrl: string, phone: string, code: string = '', password: string = ''): Promise<LocalAccountTokens> {
    const trimmedCode = code.trim();
    const hasCode = trimmedCode.length > 0;
    const hasPassword = password.length > 0;
    if (hasCode === hasPassword) {
      throw new Error('Provide exactly one of verification code or password.');
    }
    const body: LocalLoginWireRequest = { phone };
    if (hasCode) body.code = trimmedCode; else body.password = password;
    return toLocalTokens(await this.post<LocalTokenPairWire>(relayUrl, '/api/auth/local/login', body));
  }

  /**
   * `POST /api/auth/local/refresh` — rotate the relay-local token pair.
   *
   * A 401 means the refresh family itself expired; the caller falls back to
   * the sign-in form. The response user view matches `localLogin`.
   */
  async localRefresh(relayUrl: string, refreshToken: string): Promise<LocalAccountTokens> {
    const body: LocalRefreshWireRequest = { refreshToken };
    return toLocalTokens(await this.post<LocalTokenPairWire>(relayUrl, '/api/auth/local/refresh', body));
  }

  /**
   * `POST /api/auth/login` — exchange a relay access token for this device's
   * session.
   *
   * [deviceSecret] is the persisted X25519 device key and is never
   * regenerated: the relay binds this phone's device row to that public key,
   * so a fresh key would register a new device and strand the account's
   * desktops. The returned master key aliases the same secret.
   */
  async exchangeDeviceSession(
    relayUrl: string,
    accessToken: string,
    deviceId: string,
    deviceSecret: Uint8Array
  ): Promise<CloudAccountSession> {
    if (deviceSecret.length !== 32) throw new Error('Invalid device identity.');
    const keys: DeviceKeyPair = { privateKey: new Uint8Array(deviceSecret), publicKey: X25519.scalarMultBase(deviceSecret) };
    const body: LoginRequest = {
      access_token: accessToken, device_id: deviceId, device_name: 'HarmonyOS Phone',
      device_kind: DEVICE_KIND_MOBILE, public_key: Encoding.bytesToBase64(keys.publicKey),
      request_id: Encoding.randomId('').slice(1),
      clientVersion: clientVersion(), clientProtocol: CLIENT_PROTOCOL_VERSION
    };
    try {
      const auth = await this.post<AccountAuthResponse>(relayUrl, '/api/auth/login', body);
      if (!auth.token || !auth.user_id) throw new Error('Invalid account identity.');
      return { token: auth.token, userId: auth.user_id, masterKey: keys.privateKey };
    } catch (err) {
      keys.privateKey.fill(0);
      throw OhosError.wrap('local sign-in', err);
    }
  }

  async provisionDevice(
    relayUrl: string,
    session: CloudAccountSession,
    deviceId: string,
    deviceName: string,
    requestId: string,
    deviceSecret?: Uint8Array
  ): Promise<CloudProvisionedDevice> {
    const privateKey = deviceSecret ? Encoding.copyBytes(deviceSecret) : X25519.generateKeyPair().privateKey;
    try {
      if (privateKey.length !== 32) throw new Error('Invalid watch device identity.');
      const body: ProvisionDeviceRequest = {
        public_key: Encoding.bytesToBase64(X25519.scalarMultBase(privateKey)),
        device_id: deviceId,
        device_name: deviceName,
        device_kind: DEVICE_KIND_WATCH,
        request_id: requestId
      };
      const wire = await this.request<ProvisionDeviceWire>(
        relayUrl, '/api/auth/provision-device', 'POST', body, session.token);
      const token = (wire.token || '').trim();
      const userId = (wire.user_id || '').trim();
      const provisionedDeviceId = (wire.device_id || '').trim();
      if (token.length === 0 || userId !== session.userId || provisionedDeviceId !== deviceId) {
        // A credential naming a different account or device would sign the watch
        // in as somebody else. Refuse rather than pass it on.
        throw new Error('Relay returned a mismatched provisioned device identity.');
      }
      return { token, userId, deviceId: provisionedDeviceId, deviceSecret: privateKey };
    } catch (err) {
      privateKey.fill(0);
      throw err instanceof Error ? err : new Error('Watch credential provisioning failed.');
    }
  }

  /**
   * The account's controllable devices — desktops only.
   *
   * A relay that stores device kinds already filters this list; the client
   * repeats the judgement so a phone stops listing itself and its peers before
   * that relay is deployed. [selfDeviceId] is this install's own device id.
   */
  async listDevices(
    relayUrl: string,
    session: CloudAccountSession,
    selfDeviceId: string = ''
  ): Promise<CloudAccountDevice[]> {
    const devices = await this.request<CloudAccountDeviceWire[]>(relayUrl, '/api/devices', 'GET', undefined, session.token);
    const hosts = devices.filter((device: CloudAccountDeviceWire): boolean =>
      isHostDeviceRow(device, selfDeviceId));
    return hosts.map((device: CloudAccountDeviceWire): CloudAccountDevice => ({
      deviceId: device.device_id,
      deviceName: device.device_name || device.device_id,
      deviceKind: device.device_kind,
      online: device.online,
      lastSeenAt: device.last_seen_at,
      compatible: device.compatible
    }));
  }

  /**
   * [timeoutMs] defaults to the relay's own patience for a device that is slow
   * to answer. A caller with a reason to give up earlier — a handshake, where
   * silence means the desktop is gone rather than busy — passes its own.
   */
  async deviceRpc<T extends CommandStatusResponse>(
    relayUrl: string,
    session: CloudAccountSession,
    targetDeviceId: string,
    command: RemoteCommand,
    timeoutMs: number = 130000
  ): Promise<T> {
    const target = targetDeviceId.trim();
    if (target.length === 0) {
      throw new Error('Remote target device is required.');
    }
    const socket = this.connection(relayUrl, session.token);
    const messageKey = await this.peerMessageKey(relayUrl, session, target);
    const nonce = Encoding.randomBytes(12);
    const encrypted = await this.cipher.encrypt(
      Encoding.utf8ToBytes(JSON.stringify(command)),
      messageKey,
      nonce
    );
    const body: EncryptedPayload = {
      encrypted_data: Encoding.bytesToBase64(encrypted),
      nonce: Encoding.bytesToBase64(nonce)
    };
    RemoteLogger.info(`device rpc cmd=${command.cmd || 'unknown'} bytes=${body.encrypted_data.length}`);
    const response = await socket.call(target, body, timeoutMs);
    let plain: Uint8Array;
    try {
      plain = await this.cipher.decrypt(
        Encoding.base64ToBytes(response.encrypted_data),
        messageKey,
        Encoding.base64ToBytes(response.nonce)
      );
    } catch (err) {
      // The peer may have re-registered with a new key; the next call re-derives.
      this.forgetPeerKey(session, target);
      throw OhosError.wrap('device rpc decrypt', err);
    }
    const parsed = Encoding.parseJsonObject<T>(Encoding.bytesToUtf8(plain));
    if (parsed.resp === 'error') {
      throw new Error(parsed.message || 'Remote device command failed.');
    }
    return parsed;
  }

  observePresence(relayUrl: string, session: CloudAccountSession,
    onPresence: (ids: string[]) => void, onReconnect: () => void): () => void {
    const socket = this.connection(relayUrl, session.token);
    const stopUpdate = socket.onUpdate((event: string, data: Object) => {
      const presence = data as AccountPresence;
      if (event === 'ephemeral' && presence.type === 'device-presence') {
        onPresence((presence.devices || []).map((device: AccountPresenceDevice) => device.device_id));
      }
    });
    const stopReconnect = socket.onReconnect(onReconnect);
    return () => { stopUpdate(); stopReconnect(); };
  }

  /**
   * Opens one host-owned stream (a session transcript, a terminal, or the
   * catalog) on the desktop [targetId]. Pages travel as pairwise-encrypted
   * `read_stream` RPCs and change hints as `host-stream-changed` device
   * events, both forwarded by the relay without being stored. Nothing is
   * cached on the phone; an offline desktop has no history to show.
   */
  subscribeSession(relayUrl: string, session: CloudAccountSession, targetId: string,
    streamId: string, callbacks: HostSessionCallbacks): HostSessionStream {
    const target = targetId.trim();
    const source: HostSessionSource = {
      target: target,
      read: (request: HostStreamReadRequest): Promise<HostStreamPage> => {
        const command: RemoteCommand = { cmd: 'read_stream', stream_id: streamId, subscribe: true };
        if (request.after !== undefined) command.after = request.after;
        if (request.before !== undefined) command.before = request.before;
        if (request.epoch !== undefined) command.epoch = request.epoch;
        return this.deviceRpc<HostStreamPage>(relayUrl, session, target, command, 30000);
      },
      unsubscribe: async (): Promise<void> => {
        await this.deviceRpc<CommandStatusResponse>(relayUrl, session, target,
          { cmd: 'unsubscribe_stream', stream_id: streamId }, 15000);
      },
      onHint: (listener: (hint: HostStreamHint) => void): (() => void) =>
        this.observeStreamHints(relayUrl, session, target, listener),
      onReconnect: (listener: () => void): (() => void) => this.connection(relayUrl, session.token).onReconnect(listener)
    };
    return new HostSessionStream(streamId, source, callbacks);
  }

  /** Decrypts `device-event` frames from [target] and forwards the stream hints among them. */
  private observeStreamHints(relayUrl: string, session: CloudAccountSession, target: string,
    listener: (hint: HostStreamHint) => void): () => void {
    const socket = this.connection(relayUrl, session.token);
    return socket.onUpdate((event: string, data: Object) => {
      const frame = data as DeviceEventFrame;
      if (event !== 'ephemeral' || frame.type !== 'device-event') return;
      if (frame.sourceDeviceId !== target || !frame.params) return;
      void this.decodeStreamHint(relayUrl, session, target, frame.params).then((hint?: HostStreamHint) => {
        if (hint) listener(hint);
      });
    });
  }

  private async decodeStreamHint(relayUrl: string, session: CloudAccountSession, target: string,
    params: EncryptedPayload): Promise<HostStreamHint | undefined> {
    try {
      const key = await this.peerMessageKey(relayUrl, session, target);
      const plain = await this.cipher.decrypt(Encoding.base64ToBytes(params.encrypted_data), key,
        Encoding.base64ToBytes(params.nonce));
      const body = Encoding.parseJsonObject<HostStreamHintPlaintext>(Encoding.bytesToUtf8(plain));
      return parseHostStreamHint(target, body);
    } catch (err) {
      // A frame this key cannot open means the peer re-keyed or the frame was
      // not for us; the next RPC re-derives, and the hint is only a nudge.
      this.forgetPeerKey(session, target);
      RemoteLogger.warn(`device event ignored: ${OhosError.reason(err)}`);
      return undefined;
    }
  }

  private async post<T>(relayUrl: string, path: string, body: Object): Promise<T> {
    return this.request<T>(relayUrl, path, 'POST', body);
  }

  private async request<T>(
    relayUrl: string,
    path: string,
    method: string,
    body?: Object,
    token: string = '',
    readTimeoutMs: number = 120000
  ): Promise<T> {
    const request = http.createHttp();
    const base = accountRelayBase(relayUrl);
    if (!base) throw new Error('Invalid Relay URL');
    // Timed so the watch has something to be slow relative to. Phone and watch
    // reach the same relay and the same desktop, so a gap between them is the
    // client's own cost and a shared floor is the round trip's.
    const startedAt = Date.now();
    try {
      const headers: Record<string, string> = { 'Content-Type': 'application/json', 'Accept': 'application/json' };
      if (token.length > 0) headers.Authorization = `Bearer ${token}`;
      const requestOptions: http.HttpRequestOptions = {
        method: method === 'GET' ? http.RequestMethod.GET : method === 'DELETE' ? http.RequestMethod.DELETE : http.RequestMethod.POST,
        header: headers,
        expectDataType: http.HttpDataType.STRING,
        connectTimeout: 15000,
        readTimeout: readTimeoutMs
      };
      let bodyBytes = 0;
      if (body !== undefined) {
        const encoded = JSON.stringify(body);
        bodyBytes = encoded.length;
        requestOptions.extraData = encoded;
      }
      const response = await request.request(`${base}${path}`, requestOptions);
      const text = typeof response.result === 'string' ? response.result : JSON.stringify(response.result);
      // `tls` is cumulative from request start: near zero means the connection
      // was reused, and anything large is a handshake this call paid for alone.
      const timing = response.performanceTiming;
      RemoteLogger.info(`relay ${method} ${path} status=${response.responseCode} elapsed_ms=${Date.now() - startedAt}` +
        ` dns=${Math.round(timing.dnsTiming)} tcp=${Math.round(timing.tcpTiming)}` +
        ` tls=${Math.round(timing.tlsTiming)} ttfb=${Math.round(timing.firstReceiveTiming)}`);
      if (response.responseCode < 200 || response.responseCode >= 300) {
        // The size belongs in the log: a body the relay refuses for being too
        // big is otherwise indistinguishable from one it refuses for auth.
        RemoteLogger.error(`relay request failed ${method} ${path} status=${response.responseCode} bytes=${bodyBytes}`);
        let message = response.responseCode === 413 ? RemoteI18n.t('errors.payloadTooLarge') :
          `Relay request failed (HTTP ${response.responseCode}).`;
        let retryAfterSecs: number | undefined = undefined;
        try {
          const error = JSON.parse(text) as RelayErrorResponse;
          if (error.error) message = error.error;
          if (typeof error.retry_after_secs === 'number' && error.retry_after_secs > 0) {
            retryAfterSecs = Math.ceil(error.retry_after_secs);
          }
        } catch (_err) {
          // Keep the status-based error when the relay did not return JSON.
        }
        throw new CloudAccountRequestError(message, response.responseCode, retryAfterSecs);
      }
      return JSON.parse(text) as T;
    } catch (err) {
      const code = (err as NetworkRequestFailure).code;
      if (typeof code === 'number') {
        RemoteLogger.warn(`relay transport failed method=${method} path=${path} code=${code}`);
        throw new CloudAccountTransportError(code);
      }
      throw OhosError.wrap('relay request', err);
    } finally {
      request.destroy();
    }
  }
}

interface AccountPresenceDevice { device_id: string; }
interface AccountPresence { type: string; devices?: AccountPresenceDevice[]; }
