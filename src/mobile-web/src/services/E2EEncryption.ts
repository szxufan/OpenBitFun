import { hkdf } from '@noble/hashes/hkdf.js';
import { sha256 } from '@noble/hashes/sha2.js';
/**
 * E2E encryption for the mobile web client.
 *
 * Uses pure-JS implementations that work on plain HTTP pages:
 *   - X25519 key exchange: @noble/curves
 *   - AES-256-GCM:         @noble/ciphers
 *
 * The shared secret (32 bytes from X25519) is used directly as the AES key,
 * matching the Rust desktop side which uses x25519-dalek + aes-gcm the same way.
 */

// @ts-ignore — @noble/curves v2 exports with .js suffix in package.json
import { x25519 } from '@noble/curves/ed25519.js';
// @ts-ignore — @noble/ciphers v2 exports with .js suffix in package.json
import { gcm } from '@noble/ciphers/aes.js';

const NONCE_LEN = 12;

export interface MobileKeyPair {
  publicKey: Uint8Array;
  privateKey: Uint8Array;
}

export async function generateKeyPair(): Promise<MobileKeyPair> {
  const priv = randomBytes(32);
  const pub = x25519.getPublicKey(priv);
  return { publicKey: new Uint8Array(pub), privateKey: priv };
}

export async function deriveSharedKey(
  kp: MobileKeyPair,
  peerPub: Uint8Array,
): Promise<Uint8Array> {
  const shared = x25519.getSharedSecret(kp.privateKey, peerPub);
  return new Uint8Array(shared);
}

export async function encrypt(
  key: Uint8Array,
  plaintext: string,
): Promise<{ data: string; nonce: string }> {
  const nonce = randomBytes(NONCE_LEN);
  const cipher = gcm(key, nonce);
  const ct = cipher.encrypt(new TextEncoder().encode(plaintext));
  return { data: toB64(ct), nonce: toB64(nonce) };
}

export async function decrypt(
  key: Uint8Array,
  dataB64: string,
  nonceB64: string,
): Promise<string> {
  const pt = decryptBytes(key, fromB64(dataB64), fromB64(nonceB64));
  return new TextDecoder().decode(pt);
}

/** Decrypt a raw AES-256-GCM payload, including its appended authentication tag. */
export function decryptBytes(
  key: Uint8Array,
  data: Uint8Array,
  nonce: Uint8Array,
): Uint8Array {
  return gcm(key, nonce).decrypt(data);
}

export function toB64(buf: Uint8Array): string {
  let s = '';
  buf.forEach(b => (s += String.fromCharCode(b)));
  return btoa(s);
}

export function fromB64(b64: string): Uint8Array {
  const s = atob(b64);
  const buf = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) buf[i] = s.charCodeAt(i);
  return buf;
}

function randomBytes(len: number): Uint8Array {
  const buf = new Uint8Array(len);
  // crypto.getRandomValues works on HTTP too (it's not part of subtle)
  (globalThis.crypto || (globalThis as any).msCrypto).getRandomValues(buf);
  return buf;
}

/** Matches the Relay v1 device key contract used by desktop and CLI. */
export function deriveDeviceMessageKey(privateKey: Uint8Array, peerPublicKey: Uint8Array): Uint8Array {
  if (privateKey.length !== 32 || peerPublicKey.length !== 32) throw new Error('Invalid device key.');
  const ownPublicKey = x25519.getPublicKey(privateKey);
  const shared = x25519.getSharedSecret(privateKey, peerPublicKey);
  if (shared.every((value: number) => value === 0)) throw new Error('Invalid peer key.');
  let first = ownPublicKey;
  let second = peerPublicKey;
  for (let i = 0; i < 32; i += 1) {
    if (ownPublicKey[i] === peerPublicKey[i]) continue;
    if (ownPublicKey[i] > peerPublicKey[i]) [first, second] = [second, first];
    break;
  }
  const info = new Uint8Array(64);
  info.set(first); info.set(second, 32);
  try {
    return hkdf(sha256, shared, new TextEncoder().encode('OpenBitFun Relay v1.0.0 device key'), info, 32);
  } finally { shared.fill(0); }
}

/** Domain-separated salt for the PSK-paired derivation; mirrors
 * `PSK_KDF_SALT` in `remote_connect/device_crypto.rs`. */
const PSK_KDF_SALT = 'OpenBitFun Relay v1.1.0 device key psk';
/** Wire prefix of the optional envelope `key_id`; mirrors `PSK_KEY_ID_PREFIX`. */
export const PSK_KEY_ID_PREFIX = 'psk_v1:';

/** Short stable identifier of a pairing secret; mirrors `device_crypto::psk_id`. */
export function pskIdOf(psk: Uint8Array): Uint8Array {
  if (psk.length !== 32) throw new Error('Invalid pairing key.');
  const hasher = sha256.create();
  hasher.update(new TextEncoder().encode('OpenBitFun PSK id v1'));
  hasher.update(psk);
  return new Uint8Array(hasher.digest().slice(0, 8));
}

/** The `key_id` wire value a paired sender attaches; mirrors `psk_key_id`. */
export function pskKeyIdOf(psk: Uint8Array): string {
  return Array.from(pskIdOf(psk), byte => byte.toString(16).padStart(2, '0')).join('');
}

/** PSK-paired variant of `deriveDeviceMessageKey`; mirrors
 * `device_crypto::derive_message_key_psk`. The out-of-band pairing secret is
 * concatenated with the X25519 shared secret as HKDF input, so a relay that
 * swaps directory public keys cannot derive the message key. */
export function deriveDeviceMessageKeyPsk(
  privateKey: Uint8Array,
  peerPublicKey: Uint8Array,
  psk: Uint8Array,
): Uint8Array {
  if (privateKey.length !== 32 || peerPublicKey.length !== 32) throw new Error('Invalid device key.');
  if (psk.length !== 32) throw new Error('Invalid pairing key.');
  const ownPublicKey = x25519.getPublicKey(privateKey);
  const shared = x25519.getSharedSecret(privateKey, peerPublicKey);
  if (shared.every((value: number) => value === 0)) throw new Error('Invalid peer key.');
  let first = ownPublicKey;
  let second = peerPublicKey;
  for (let i = 0; i < 32; i += 1) {
    if (ownPublicKey[i] === peerPublicKey[i]) continue;
    if (ownPublicKey[i] > peerPublicKey[i]) [first, second] = [second, first];
    break;
  }
  const pskId = pskIdOf(psk);
  const info = new Uint8Array(72);
  info.set(first); info.set(second, 32); info.set(pskId, 64);
  const ikm = new Uint8Array(64);
  ikm.set(shared); ikm.set(psk, 32);
  try {
    return hkdf(sha256, ikm, new TextEncoder().encode(PSK_KDF_SALT), info, 32);
  } finally { shared.fill(0); ikm.fill(0); }
}
