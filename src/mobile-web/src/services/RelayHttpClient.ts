import {
  openHostStream, parseStreamHint, parseStreamPage,
  type HostStreamOptions, type SessionStreamHandle, type StreamHint, type StreamReadRequest,
} from '../../../shared/relay-transport/HostStream';
/** Account directory plus the shared Socket.IO encrypted RPC transport. */
import { AccountRealtime, type DeviceEventEnvelope } from '../../../shared/relay-transport/AccountRealtime';
import { PSK_KEY_MISMATCH_MESSAGE } from '../../../shared/relay-transport/HostStream';
import {
  deriveDeviceMessageKey, deriveDeviceMessageKeyPsk, encrypt, decrypt, fromB64, PSK_KEY_ID_PREFIX,
} from './E2EEncryption';
import { PeerPskStore } from './PeerPskStore';
import { parsePskPairingCode } from './PskPairing';
import { normalizeRelayUrl } from './pairingLink';

export interface AccountIdentity {
  token: string;
  masterKey: Uint8Array;
  userId: string;
  deviceId: string;
}
interface AccountIdentitySnapshot extends AccountIdentity { generation: number; }
export type AccountOwnerChange = {
  kind: 'initial' | 'replacement' | 'unavailable';
  epoch: number;
  userId: string | null;
};
export type ControlTargetSnapshot = Readonly<{ deviceId: string | null; epoch: number }>;
export class AccountIdentityChangedError extends Error {
  constructor() { super('Account identity changed'); this.name = 'AccountIdentityChangedError'; }
}
export function isAccountIdentityChangedError(value: unknown): value is AccountIdentityChangedError {
  return value instanceof AccountIdentityChangedError;
}
const RELAY_HTTP_MAX_ATTEMPTS = 5;
const RELAY_HTTP_RETRY_BASE_DELAY_MS = 300;
const RELAY_HTTP_RETRY_BUDGET_MS = 120_000;
const TRANSIENT_RELAY_STATUSES = new Set([408, 425, 500, 502, 503, 504]);
type RelayRequestOptions = { retryable?: boolean; timeoutMs?: number };

export interface RelayDeviceInfo {
  device_id: string;
  device_name: string;
  /**
   * Kind the device reported to the Relay: `desktop`, `cli`, `mobile` or
   * `watch`. Absent on an older Relay or a client that never reported one, so
   * every reader has to keep a neutral answer for "unknown".
   */
  device_kind?: string | null;
  device_alias?: string | null;
  device_model?: string | null;
  device_os?: string | null;
  device_os_version?: string | null;
  /** Client build the device reported to the Relay. Absent on older Relays. */
  client_version?: string | null;
  /** Client wire protocol the device reported. Absent on older Relays. */
  client_protocol?: number | null;
  /**
   * Relay-computed mutual-control compatibility of this device with this one.
   *
   * `false` means confirmed incompatible: either a client build/protocol
   * mismatch or a peer that reported no version information (an older client).
   * Absent only on an older Relay that does not gate at all, which must be
   * treated as "unknown but usable", never as incompatible.
   */
  compatible?: boolean;
  online: boolean;
  last_seen_at?: number | null;
}

export function deviceDisplayName(device: Pick<RelayDeviceInfo, 'device_id' | 'device_name' | 'device_alias'>): string {
  return device.device_alias ?? device.device_name ?? device.device_id;
}


export class RelayHttpClient {
  private readonly relayUrl: string;
  private realtime: AccountRealtime | null = null;
  private identity: AccountIdentitySnapshot | null = null;
  private identityGeneration = 0;
  private accountEpochValue = 0;
  private directoryRequest = 0;
  private appliedDirectoryRequest = 0;
  private directorySnapshot: RelayDeviceInfo[] = [];
  private directorySnapshotListeners = new Set<(devices: RelayDeviceInfo[]) => void>();
  onDeviceDirectorySnapshot(listener: (devices: RelayDeviceInfo[]) => void): () => void {
    this.directorySnapshotListeners.add(listener);
    return () => { this.directorySnapshotListeners.delete(listener); };
  }
  resolveDeviceName(deviceId: string, fallback = deviceId): string {
    const device = this.directorySnapshot.find(item => item.device_id === deviceId);
    return device ? deviceDisplayName(device) : fallback;
  }
  private directoryListeners = new Set<() => void>();
  private ownerListeners = new Set<(change: AccountOwnerChange) => void>();
  private authorizationExpiredListeners = new Set<(token: string) => void>();
  private targetDeviceIdValue: string | null = null;
  private controlTargetEpochValue = 0;
  private controlTargetListeners = new Set<(snapshot: ControlTargetSnapshot) => void>();
  private deviceMessageKeys = new Map<string, { expires: number; key: Promise<Uint8Array> }>();
  private streamHintListeners = new Set<(hint: StreamHint) => void>();

  constructor(relayUrl: string, identity: AccountIdentity) {
    const endpoint = normalizeRelayUrl(relayUrl);
    if (!endpoint) throw new Error('Invalid Relay URL');
    this.relayUrl = endpoint;
    this.setAccountIdentity(identity);
  }

  setAccountIdentity(identity: AccountIdentity): void {
    if (!identity.token.trim() || !identity.userId.trim() || !identity.deviceId.trim() || identity.masterKey.length !== 32) {
      throw new Error('Relay returned an invalid account identity.');
    }
    const kind = this.identity ? 'replacement' : 'initial';
    this.realtime?.close();
    this.realtime = null;
    this.identity?.masterKey.fill(0);
    this.identity = { ...identity, masterKey: identity.masterKey.slice(), generation: ++this.identityGeneration };
    this.realtime = new AccountRealtime({ url: this.relayUrl, token: identity.token });
    const notifyDirectory = () => { for (const listener of this.directoryListeners) listener(); };
    this.realtime.onDeviceDirectoryChanged(notifyDirectory);
    this.realtime.onReconnect(notifyDirectory);
    this.realtime.onDeviceEvent(envelope => { void this.receiveDeviceEvent(envelope); });
    this.accountEpochValue += 1;
    this.directorySnapshot = [];
    this.directoryRequest += 1;
    // A read that started under the previous identity must not seed the cache.
    this.appliedDirectoryRequest = this.directoryRequest;
    this.deviceMessageKeys.clear();
    this.setTargetDeviceId(null);
    for (const listener of this.ownerListeners) listener({ kind, epoch: this.accountEpochValue, userId: identity.userId });
  }

  /**
   * Swap the account token in place after a relay-local refresh: same user,
   * same controller device, so no owner change is announced and the selected
   * control target survives. Work still in flight under the previous token
   * fails with `AccountIdentityChangedError`, exactly like a replacement.
   */
  rotateAccountToken(token: string): void {
    const identity = this.identity;
    if (!identity || !token.trim()) throw new Error('Relay returned an invalid account identity.');
    if (identity.token === token) return;
    this.realtime?.close();
    this.realtime = null;
    this.identity = { ...identity, token, generation: ++this.identityGeneration };
    this.realtime = new AccountRealtime({ url: this.relayUrl, token });
    const notifyDirectory = () => { for (const listener of this.directoryListeners) listener(); };
    this.realtime.onDeviceDirectoryChanged(notifyDirectory);
    this.realtime.onReconnect(notifyDirectory);
    this.realtime.onDeviceEvent(envelope => { void this.receiveDeviceEvent(envelope); });
    this.deviceMessageKeys.clear();
  }

  resetConnectionIdentity(): void {
    this.realtime?.close();
    this.realtime = null;
    this.identity?.masterKey.fill(0);
    this.identity = null;
    this.identityGeneration += 1;
    this.accountEpochValue += 1;
    this.directorySnapshot = [];
    this.directoryRequest += 1;
    // A read from the closed identity must not seed the cache either.
    this.appliedDirectoryRequest = this.directoryRequest;
    this.deviceMessageKeys.clear();
    this.setTargetDeviceId(null);
    for (const listener of this.ownerListeners) listener({ kind: 'unavailable', epoch: this.accountEpochValue, userId: null });
  }

  onDeviceDirectoryChanged(listener: () => void): () => void {
    this.directoryListeners.add(listener);
    return () => { this.directoryListeners.delete(listener); };
  }

  onAccountOwnerChange(listener: (change: AccountOwnerChange) => void, options?: { emitCurrent?: boolean }): () => void {
    this.ownerListeners.add(listener);
    if (options?.emitCurrent && this.identity) listener({ kind: 'initial', epoch: this.accountEpochValue, userId: this.identity.userId });
    return () => this.ownerListeners.delete(listener);
  }
  onAuthorizationExpired(listener: (token: string) => void): () => void {
    this.authorizationExpiredListeners.add(listener);
    return () => this.authorizationExpiredListeners.delete(listener);
  }
  get hasAccountIdentity(): boolean { return this.identity !== null; }
  get accountEpoch(): number { return this.accountEpochValue; }
  get accountUserId(): string | null { return this.identity?.userId ?? null; }
  get controllerDeviceId(): string | null { return this.identity?.deviceId ?? null; }
  get targetDeviceId(): string | null { return this.targetDeviceIdValue; }
  get controlTargetEpoch(): number { return this.controlTargetEpochValue; }
  setTargetDeviceId(deviceId: string | null): void {
    this.targetDeviceIdValue = deviceId;
    this.controlTargetEpochValue += 1;
    const snapshot = this.getControlTargetSnapshot();
    for (const listener of this.controlTargetListeners) listener(snapshot);
  }
  getControlTargetSnapshot(): ControlTargetSnapshot {
    return { deviceId: this.targetDeviceIdValue, epoch: this.controlTargetEpochValue };
  }
  isControlTargetCurrent(snapshot: ControlTargetSnapshot): boolean { return snapshot.epoch === this.controlTargetEpochValue; }
  onControlTargetChange(listener: (snapshot: ControlTargetSnapshot) => void): () => void {
    this.controlTargetListeners.add(listener);
    return () => this.controlTargetListeners.delete(listener);
  }

  /** Stream hints decrypted from the controlled host; only its device is trusted. */
  onStreamHint(listener: (hint: StreamHint) => void): () => void {
    this.streamHintListeners.add(listener);
    return () => { this.streamHintListeners.delete(listener); };
  }

  /** Decrypt a forwarded `DeviceEvent`. Only the selected control target is a
   * hint source; events from any other device are dropped before key lookup. */
  private async receiveDeviceEvent(envelope: DeviceEventEnvelope): Promise<void> {
    const identity = this.identity;
    const target = this.getControlTargetSnapshot();
    if (!identity || envelope.sourceDeviceId !== target.deviceId) return;
    try {
      const messageKey = await this.receiverMessageKey(identity, envelope.sourceDeviceId, envelope.key_id);
      if (this.identity !== identity || !this.isControlTargetCurrent(target)) return;
      const plaintext = JSON.parse(await decrypt(messageKey, envelope.encrypted_data, envelope.nonce)) as { cmd?: unknown; event?: unknown; payload?: unknown };
      if (plaintext?.cmd !== 'device_event' || typeof plaintext.event !== 'string') return;
      const hint = parseStreamHint(envelope.sourceDeviceId, plaintext.event, plaintext.payload);
      if (hint) for (const listener of this.streamHintListeners) listener(hint);
    } catch (error) {
      console.warn('[RelayHttpClient] device event ignored', error);
    }
  }

  /** Read one host-owned stream (session records, terminal output hints, host
   * catalog) directly from the selected control target. Nothing is cached. */
  async subscribeHostStream(streamId: string, callbacks: Pick<HostStreamOptions, 'onEvent' | 'onError' | 'onCaughtUp' | 'onHistoryState' | 'onResumed' | 'onGap'>): Promise<SessionStreamHandle> {
    const identity = this.identity;
    const connection = this.realtime;
    const target = this.getControlTargetSnapshot();
    if (!identity || !connection || !target.deviceId) throw new Error('No controlled runtime is selected');
    const deviceId = target.deviceId;
    let disposed = false;
    let stop: SessionStreamHandle | undefined;
    const current = () => !disposed && this.identity === identity && this.isControlTargetCurrent(target);
    const dispose = () => { disposed = true; stop?.close(); unbindTarget(); unbindOwner(); };
    const unbindTarget = this.onControlTargetChange(dispose);
    const unbindOwner = this.onAccountOwnerChange(dispose);
    const transport = {
      readStream: async (request: StreamReadRequest) => {
        if (!current()) throw new AccountIdentityChangedError();
        return parseStreamPage(await this.sendDeviceRpc(deviceId, { cmd: 'read_stream', ...request }, { retryable: true, timeoutMs: 30_000 }));
      },
      unsubscribeStream: async (id: string) => {
        // Best effort: the host also drops the lease when this device goes
        // offline or stops renewing.
        if (this.identity !== identity) return;
        await this.sendDeviceRpc(deviceId, { cmd: 'unsubscribe_stream', stream_id: id }, { retryable: true, timeoutMs: 10_000 });
      },
    };
    const signals = {
      onHint: (listener: (hint: StreamHint) => void) => this.onStreamHint(listener),
      onReconnect: (listener: () => void) => connection.onReconnect(listener),
    };
    try {
      stop = await openHostStream({ transport, signals, target: deviceId, streamId,
        onEvent: event => { if (current()) callbacks.onEvent(event); },
        onError: error => { if (current()) callbacks.onError(error); },
        onCaughtUp: () => { if (current()) callbacks.onCaughtUp?.(); },
        onHistoryState: state => { if (current()) callbacks.onHistoryState?.(state); },
        onResumed: () => { if (current()) callbacks.onResumed?.(); },
        onGap: reason => { if (current()) callbacks.onGap?.(reason); },
      });
      if (!current()) dispose();
      return { close: dispose, wake: () => { if (current()) stop?.wake(); }, loadOlder: () => current() && stop ? stop.loadOlder() : Promise.resolve() };
    } catch (error) { dispose(); throw error; }
  }

  private async fetchWithTimeout(
    input: RequestInfo | URL,
    init: RequestInit,
    timeoutMs: number,
  ): Promise<Response> {
    const controller = new AbortController();
    const timer = window.setTimeout(() => controller.abort(), timeoutMs);
    try {
      const response = await fetch(input, { ...init, signal: controller.signal });
      // Keep the deadline active until the full body has been received.
      // `fetch()` resolves after response headers, so returning that Response
      // directly would let a stalled body wait forever outside the timeout.
      const body = await response.arrayBuffer();
      return new Response(body, {
        status: response.status,
        statusText: response.statusText,
        headers: response.headers,
      });
    } catch (error: unknown) {
      if ((error as { name?: string })?.name === 'AbortError') {
        throw new Error('Request timed out');
      }
      throw error;
    } finally {
      window.clearTimeout(timer);
    }
  }

  private async fetchWithRetry(
    input: RequestInfo | URL,
    init: RequestInit,
    timeoutMs: number,
    assertCurrent: () => void = () => {},
  ): Promise<Response> {
    let lastError: unknown = null;
    const deadlineMs = Date.now() + RELAY_HTTP_RETRY_BUDGET_MS;
    for (let attempt = 1; attempt <= RELAY_HTTP_MAX_ATTEMPTS; attempt += 1) {
      assertCurrent();
      try {
        const remainingMs = deadlineMs - Date.now();
        if (remainingMs <= 0) {
          throw lastError ?? new Error('Relay request retry budget exceeded');
        }
        const response = await this.fetchWithTimeout(
          input,
          init,
          Math.min(timeoutMs, remainingMs),
        );
        if (
          TRANSIENT_RELAY_STATUSES.has(response.status)
          && attempt < RELAY_HTTP_MAX_ATTEMPTS
        ) {
          lastError = new Error(`Relay returned HTTP ${response.status}`);
          void response.body?.cancel();
        } else {
          return response;
        }
      } catch (error) {
        lastError = error;
        if (attempt === RELAY_HTTP_MAX_ATTEMPTS) throw error;
      }
      const delayMs = RELAY_HTTP_RETRY_BASE_DELAY_MS * (2 ** (attempt - 1));
      if (Date.now() + delayMs >= deadlineMs) {
        throw lastError ?? new Error('Relay request retry budget exceeded');
      }
      await new Promise(resolve => window.setTimeout(resolve, delayMs));
    }
    throw lastError;
  }

  async listDevices(): Promise<RelayDeviceInfo[]> {
    const request = ++this.directoryRequest;
    return this.withAccount(async (identity) => {
      const resp = await this.fetchWithRetry(`${this.relayUrl}/api/devices`, {
        headers: { 'Authorization': `Bearer ${identity.token}` },
      }, 20_000, () => {
        if (identity.generation !== this.identityGeneration) throw new AccountIdentityChangedError();
      });
      if (!resp.ok) {
        const err = new Error(`List devices failed: HTTP ${resp.status}`) as Error & {
          status?: number;
        };
        err.status = resp.status;
        throw err;
      }
      const devices = await resp.json() as RelayDeviceInfo[];
      if (identity.generation !== this.identityGeneration) throw new AccountIdentityChangedError();
      // Several surfaces read the directory at once (this app's mount effect, the
      // device page, the compact session list). A read that another read
      // superseded answers with the newest completion instead of failing its
      // caller: a spurious failure here leaves that surface on an empty device
      // list until its next poll, which is exactly what a second tab used to do.
      if (request < this.appliedDirectoryRequest) return this.directorySnapshot;
      this.appliedDirectoryRequest = request;
      this.directorySnapshot = devices;
      for (const listener of this.directorySnapshotListeners) listener(devices);
      return devices;
    });
  }

  async supportsDeviceAlias(): Promise<boolean> {
    const response = await this.fetchWithTimeout(`${this.relayUrl}/api/info`, {}, 20_000);
    if (response.status === 404 || response.status === 405) return false;
    if (!response.ok) throw new Error(`Relay info failed: HTTP ${response.status}`);
    const info = await response.json() as { capabilities?: string[] };
    return Array.isArray(info.capabilities) && info.capabilities.includes('device_alias_v1');
  }

  async updateDeviceAlias(deviceId: string, alias: string | null): Promise<void> {
    await this.withAccount(async identity => {
      if (!await this.supportsDeviceAlias()) throw new Error('Device alias updates unsupported by this Relay');
      if (identity.generation !== this.identityGeneration) throw new AccountIdentityChangedError();
      const response = await this.fetchWithTimeout(`${this.relayUrl}/api/devices/${encodeURIComponent(deviceId)}`, {
        method: 'PATCH',
        headers: { Authorization: `Bearer ${identity.token}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ device_alias: alias }),
      }, 20_000);
      if (response.status === 404 || response.status === 405) throw new Error('Device alias updates unsupported or device unavailable');
      if (!response.ok) throw Object.assign(new Error(`Update device failed: HTTP ${response.status}`), { status: response.status });
    });
    for (const listener of this.directoryListeners) listener();
  }

  /** Send a command encrypted with the two account devices' key agreement:
   * the PSK-paired derivation once pairing is confirmed, the legacy X25519
   * derivation otherwise. The envelope's optional `key_id` tells the receiver
   * which derivation was used. */
  async sendDeviceRpc<T = any>(
    targetDeviceId: string,
    command: object,
    options: RelayRequestOptions = {},
  ): Promise<T> {
    const targetEpoch = this.controlTargetEpochValue;
    return this.withAccount(async (identity) => {
      const selected = await this.senderMessageKey(identity, targetDeviceId);
      const plaintext = JSON.stringify(command);
      const { data: encData, nonce: encNonce } = await encrypt(
        selected.key,
        plaintext,
      );

      if (identity.generation !== this.identityGeneration || targetEpoch !== this.controlTargetEpochValue) {
        throw new AccountIdentityChangedError();
      }
      const timeoutMs = options.timeoutMs ?? (options.retryable ? 20_000 : 130_000);
      const connection = this.realtime;
      if (!connection) throw new AccountIdentityChangedError();
      const params: Record<string, string> = { encrypted_data: encData, nonce: encNonce };
      if (selected.keyId) params.key_id = selected.keyId;
      const data = await connection.call<{ encrypted_data: string; nonce: string; key_id?: string }>(targetDeviceId,
        params, {
          timeoutMs,
          beforeSend: () => {
            if (identity.generation !== this.identityGeneration || targetEpoch !== this.controlTargetEpochValue) {
              throw new AccountIdentityChangedError();
            }
          },
        });
      if (identity.generation !== this.identityGeneration || targetEpoch !== this.controlTargetEpochValue) {
        throw new AccountIdentityChangedError();
      }
      let decrypted: string;
      try {
        decrypted = await decrypt(
          await this.receiverMessageKey(identity, targetDeviceId, data.key_id),
          data.encrypted_data,
          data.nonce,
        );
      } catch (error) {
        // A paired exchange that cannot decrypt is a pairing problem; falling
        // back to the legacy derivation would undo the pairing's protection.
        throw new Error(data.key_id ? PSK_KEY_MISMATCH_MESSAGE : (error as Error).message);
      }
      const parsed = JSON.parse(decrypted);
      if (parsed?.resp === 'error') {
        throw new Error(parsed.message || 'Remote error');
      }
      // A successful PSK decrypt is proof both sides hold the secret: promote
      // a pending offer, whatever command traveled inside the envelope.
      if (data.key_id?.startsWith(PSK_KEY_ID_PREFIX)) {
        void PeerPskStore.markPaired(
          this.relayUrl,
          targetDeviceId,
          data.key_id.slice(PSK_KEY_ID_PREFIX.length),
        ).catch(() => undefined);
      }
      return parsed as T;
    }).catch((error) => {
      this.deviceMessageKeys.clear();
      throw error;
    });
  }

  /** Accept a PSK pairing code offered by another device: verify the offerer's
   * directory key against the key embedded in the code, store the pairing,
   * and confirm over a PSK-sealed RPC. The offerer promoting its offer after
   * decrypting that envelope is the proof both sides hold the secret. */
  async completePskPairing(code: string): Promise<{ peerDeviceId: string; pskId: string; confirmed: boolean }> {
    const payload = parsePskPairingCode(code);
    return this.withAccount(async (identity) => {
      const peerPublic = await this.peerPublicKey(identity, payload.offererDeviceId);
      const embedded = payload.offererPublicKey;
      if (peerPublic.length !== embedded.length || !peerPublic.every((byte, i) => byte === embedded[i])) {
        throw new Error('Pairing refused: the offerer\'s relay directory key does not match the pairing code. The directory may be tampered with.');
      }
      const entry = await PeerPskStore.store(this.relayUrl, payload.offererDeviceId, payload.psk, 'paired');
      let confirmed = false;
      try {
        await this.sendDeviceRpc(payload.offererDeviceId, {
          cmd: 'host_invoke',
          command: 'peer_psk_confirm',
          args: { psk_id: entry.pskId, controller_device_id: identity.deviceId },
        }, { retryable: false });
        confirmed = true;
      } catch (error) {
        console.warn('[RelayHttpClient] pairing stored, confirmation not acknowledged', error);
      }
      return { peerDeviceId: payload.offererDeviceId, pskId: entry.pskId, confirmed };
    });
  }

  /** The key this controller seals with: PSK once pairing is `paired`,
   * legacy while an offer is pending or no pairing exists. */
  private async senderMessageKey(
    identity: AccountIdentitySnapshot,
    targetDeviceId: string,
  ): Promise<{ key: Uint8Array; keyId?: string }> {
    const pairing = PeerPskStore.lookup(this.relayUrl, targetDeviceId);
    if (pairing?.status === 'paired') {
      const peerPublic = await this.peerPublicKey(identity, targetDeviceId);
      return {
        key: deriveDeviceMessageKeyPsk(identity.masterKey, peerPublic, pairing.psk),
        keyId: `${PSK_KEY_ID_PREFIX}${pairing.pskId}`,
      };
    }
    return { key: await this.deviceMessageKey(identity, targetDeviceId) };
  }

  /** The key for a message a peer sealed. The envelope's `key_id` is the only
   * authority for the derivation; absent means legacy even when a pairing
   * exists, and a `key_id` without a matching stored pairing is a loud error. */
  private async receiverMessageKey(
    identity: AccountIdentitySnapshot,
    targetDeviceId: string,
    keyId: string | undefined,
  ): Promise<Uint8Array> {
    if (!keyId) return this.deviceMessageKey(identity, targetDeviceId);
    const pskId = keyId.startsWith(PSK_KEY_ID_PREFIX) ? keyId.slice(PSK_KEY_ID_PREFIX.length) : null;
    const pairing = PeerPskStore.lookup(this.relayUrl, targetDeviceId);
    if (!pskId || !pairing || pairing.pskId !== pskId) {
      throw new Error(PSK_KEY_MISMATCH_MESSAGE);
    }
    const peerPublic = await this.peerPublicKey(identity, targetDeviceId);
    return deriveDeviceMessageKeyPsk(identity.masterKey, peerPublic, pairing.psk);
  }

  /** The peer's authenticated directory public key, cached briefly. */
  private peerPublicKey(identity: AccountIdentitySnapshot, targetDeviceId: string): Promise<Uint8Array> {
    const cacheId = `${identity.generation}:${targetDeviceId}:pub`;
    let cached = this.deviceMessageKeys.get(cacheId);
    if (!cached || cached.expires < Date.now()) {
      const key = (async () => {
        const response = await this.fetchWithTimeout(
          `${this.relayUrl}/api/devices/${encodeURIComponent(targetDeviceId)}/key`,
          { headers: { Authorization: `Bearer ${identity.token}` } }, 20_000,
        );
        if (!response.ok) {
          const error = new Error(`Device key unavailable: HTTP ${response.status}`) as Error & { status?: number };
          error.status = response.status;
          throw error;
        }
        const peer = await response.json();
        if (peer.device_id !== targetDeviceId) throw new Error('Relay returned a different device identity.');
        return fromB64(peer.public_key);
      })();
      cached = { expires: Date.now() + 60_000, key };
      this.deviceMessageKeys.set(cacheId, cached);
      key.catch(() => { if (this.deviceMessageKeys.get(cacheId) === cached) this.deviceMessageKeys.delete(cacheId); });
    }
    return cached.key;
  }

  /** Pairwise X25519-derived message key for one account device, cached briefly. */
  private deviceMessageKey(identity: AccountIdentitySnapshot, targetDeviceId: string): Promise<Uint8Array> {
    const cacheId = `${identity.generation}:${targetDeviceId}`;
    let cached = this.deviceMessageKeys.get(cacheId);
    if (!cached || cached.expires < Date.now()) {
      const key = (async () => {
        const peerPublic = await this.peerPublicKey(identity, targetDeviceId);
        return deriveDeviceMessageKey(identity.masterKey, peerPublic);
      })();
      cached = { expires: Date.now() + 60_000, key };
      this.deviceMessageKeys.set(cacheId, cached);
      key.catch(() => { if (this.deviceMessageKeys.get(cacheId) === cached) this.deviceMessageKeys.delete(cacheId); });
    }
    return cached.key;
  }

  private async withAccount<T>(operation: (identity: AccountIdentitySnapshot) => Promise<T>): Promise<T> {
    const identity = this.identity;
    if (!identity) throw new Error('Sign in to continue');
    try {
      const result = await operation(identity);
      if (this.identity !== identity) throw new AccountIdentityChangedError();
      return result;
    } catch (error) {
      if (this.identity !== identity) throw new AccountIdentityChangedError();
      // Only a transport-level HTTP 401 invalidates account proof. An encrypted
      // remote tool error containing "401" must never sign the browser out.
      if ((error as { status?: number })?.status === 401) {
        for (const listener of this.authorizationExpiredListeners) listener(identity.token);
      }
      throw error;
    }
  }
}
