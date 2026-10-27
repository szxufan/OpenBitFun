import { toB64 } from './E2EEncryption';
import { x25519 } from '@noble/curves/ed25519.js';
import { CLIENT_PROTOCOL_VERSION, CLIENT_VERSION } from '../../../shared/relay-transport/ClientBuild';

import { pairingRelayUrl } from './pairingLink';
export interface CloudAccountSession { token: string; userId: string; masterKey: Uint8Array; }

/** Relay-local account profile returned together with every token pair. */
export interface LocalAccountUser {
  userId: string;
  login: string;
  phoneMasked: string;
  hasPassword: boolean;
}

/** One relay-local access/refresh pair; the access token feeds the device exchange. */
export interface LocalTokenPair {
  accessToken: string;
  refreshToken: string;
  accessExpiresAt: number;
  refreshExpiresAt: number;
  user: LocalAccountUser;
}

interface RelayErrorResponse { error?: string; retry_after_secs?: number; }
export class CloudAccountRequestError extends Error {
  readonly status: number;
  readonly retryAfterSeconds: number | null;

  constructor(message: string, status: number, retryAfterSeconds: number | null = null) {
    super(message);
    this.name = 'CloudAccountRequestError';
    this.status = status;
    this.retryAfterSeconds = retryAfterSeconds;
  }
}

/** The request never reached the relay, or its answer never came back. */
export class CloudAccountTransportError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CloudAccountTransportError';
  }
}

export function generateRequestId(): string {
  if (typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (value) => value.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/**
 * Normalize a mainland-China mobile number for the relay-local account API.
 *
 * Mirrors the Relay's own `normalize_phone` so the form can gate the send-code
 * action locally; the Relay re-validates and remains the authority.
 */
export function normalizeAccountPhone(raw: string): string | null {
  const trimmed = raw.replace(/[\s-]+/g, '');
  const digits = trimmed.startsWith('+86') ? trimmed.slice(3)
    : trimmed.startsWith('86') && trimmed.length === 13 ? trimmed.slice(2)
      : trimmed;
  return /^1[3-9]\d{9}$/.test(digits) ? digits : null;
}

async function requestJson<T>(
  relayUrl: string,
  path: string,
  body: object,
): Promise<T> {
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), 35_000);
  try {
    const response = await fetch(`${relayUrl.replace(/\/+$/, '')}${path}`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Accept: 'application/json',
      },
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    const text = await response.text();
    let parsed: T | RelayErrorResponse | null = null;
    try {
      parsed = text ? JSON.parse(text) as T | RelayErrorResponse : null;
    } catch {
      // Use the HTTP status below when a proxy returned a non-JSON body.
    }
    if (!response.ok) {
      const relayError = parsed as RelayErrorResponse | null;
      throw new CloudAccountRequestError(
        relayError?.error || `Relay request failed (HTTP ${response.status}).`,
        response.status,
        typeof relayError?.retry_after_secs === 'number'
          ? relayError.retry_after_secs
          : null,
      );
    }
    if (!parsed) throw new CloudAccountTransportError('Relay returned an empty account response.');
    return parsed as T;
  } catch (error: unknown) {
    if ((error as { name?: string })?.name === 'AbortError') {
      throw new CloudAccountTransportError('Account request timed out.');
    }
    // fetch() rejects with a TypeError for every connection-level failure:
    // offline, DNS, TLS, a dropped socket mid-body.
    if (error instanceof TypeError) {
      throw new CloudAccountTransportError('Could not reach the account service.');
    }
    throw error;
  } finally {
    window.clearTimeout(timeout);
  }
}

interface LocalTokenWire {
  accessToken?: string;
  refreshToken?: string;
  accessExpiresAt?: number;
  refreshExpiresAt?: number;
  user?: { userId?: string; login?: string; phoneMasked?: string; hasPassword?: boolean };
}

function parseLocalPair(value: LocalTokenWire): LocalTokenPair {
  const accessExpiresAt = value?.accessExpiresAt;
  const refreshExpiresAt = value?.refreshExpiresAt;
  if (!value?.accessToken?.trim() || !value?.refreshToken?.trim()
    || typeof accessExpiresAt !== 'number' || !Number.isFinite(accessExpiresAt)
    || typeof refreshExpiresAt !== 'number' || !Number.isFinite(refreshExpiresAt)
    || !value.user?.userId?.trim()) {
    throw new CloudAccountTransportError('Relay returned an incomplete local sign-in response.');
  }
  return {
    accessToken: value.accessToken,
    refreshToken: value.refreshToken,
    accessExpiresAt,
    refreshExpiresAt,
    user: {
      userId: value.user.userId,
      login: value.user.login?.trim() || value.user.userId,
      phoneMasked: value.user.phoneMasked?.trim() || '',
      hasPassword: value.user.hasPassword === true,
    },
  };
}

/** Relay-local phone accounts authorize a separately keyed browser controller. */
export class CloudAccountClient {
  private readonly relayUrl: string;
  constructor(relayUrl: string) {
    const endpoint = pairingRelayUrl(relayUrl);
    if (!endpoint) throw new Error('Invalid Relay URL');
    this.relayUrl = endpoint;
  }

  /** Whether this Relay serves the relay-local phone-account contract. */
  async supportsLocalAuth(): Promise<boolean> {
    const controller = new AbortController();
    const timer = window.setTimeout(() => controller.abort(), 10_000);
    try {
      const response = await fetch(`${this.relayUrl}/api/info`, {
        headers: { Accept: 'application/json' },
        signal: controller.signal,
      });
      if (!response.ok) return false;
      const info = await response.json() as { capabilities?: unknown };
      return Array.isArray(info.capabilities) && info.capabilities.includes('local_auth_v1');
    } catch (error: unknown) {
      if ((error as { name?: string })?.name === 'AbortError') {
        throw new CloudAccountTransportError('Account request timed out.');
      }
      // fetch() rejects with a TypeError for connection-level failures.
      if (error instanceof TypeError) {
        throw new CloudAccountTransportError('Could not reach the account service.');
      }
      throw error;
    } finally {
      window.clearTimeout(timer);
    }
  }

  /** Ask the Relay to text a verification code to this phone. A 429 answer
   * throws with the Relay's message and the cooldown in `retryAfterSeconds`. */
  async smsSend(phone: string): Promise<void> {
    await requestJson<{ status: string }>(this.relayUrl, '/api/auth/local/sms/send', { phone });
  }

  /** Sign in with an SMS code (registering the account on first use) or, for
   * accounts that set one, the optional password. Exactly one is sent. */
  async localLogin(phone: string, credentials: { code: string } | { password: string }): Promise<LocalTokenPair> {
    const body = 'code' in credentials
      ? { phone, code: credentials.code }
      : { phone, password: credentials.password };
    return parseLocalPair(await requestJson<LocalTokenWire>(this.relayUrl, '/api/auth/local/login', body));
  }

  /** Rotate a refresh token into a fresh pair; an expired or replayed token is
   * a 401 the caller must treat as "sign in again". */
  async localRefresh(refreshToken: string): Promise<LocalTokenPair> {
    return parseLocalPair(await requestJson<LocalTokenWire>(this.relayUrl, '/api/auth/local/refresh', { refreshToken }));
  }

  async login(accessToken: string, deviceId: string, browserPrivateKey: Uint8Array): Promise<CloudAccountSession> {
    if (browserPrivateKey.length !== 32) throw new Error('Stored device identity is invalid.');
    // The browser store commits one identity before any login request. Never
    // generate a new key here: concurrent tabs must register the same key.
    const keys = { privateKey: browserPrivateKey.slice(), publicKey: x25519.getPublicKey(browserPrivateKey) };
    try {
      const auth = await requestJson<{ token: string; user_id: string }>(this.relayUrl, '/api/auth/login', {
        access_token: accessToken, device_id: deviceId, device_name: 'Mobile Browser',
        device_kind: 'mobile', public_key: toB64(keys.publicKey), request_id: generateRequestId(),
        // Report the build so the Relay can gate control compatibility instead
        // of treating this browser controller as a legacy client.
        clientVersion: CLIENT_VERSION, clientProtocol: CLIENT_PROTOCOL_VERSION,
      });
      if (!auth.token?.trim() || !auth.user_id?.trim()) throw new Error('Invalid account identity.');
      return { token: auth.token, userId: auth.user_id, masterKey: keys.privateKey };
    } catch (error) {
      keys.privateKey.fill(0);
      throw error;
    }
  }

  /** Revoke only this browser token; the desktop's account remains connected. */
  async logout(token: string): Promise<void> {
    const controller = new AbortController();
    const timer = window.setTimeout(() => controller.abort(), 10_000);
    try {
      const response = await fetch(`${this.relayUrl}/api/auth/logout`, {
        method: 'POST', headers: { Authorization: `Bearer ${token}` }, signal: controller.signal,
      });
      if (!response.ok && response.status !== 401) throw new Error(`Sign-out failed: HTTP ${response.status}`);
    } finally { window.clearTimeout(timer); }
  }
}
