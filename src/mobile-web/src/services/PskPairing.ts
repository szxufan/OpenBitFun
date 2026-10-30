/**
 * Out-of-band PSK pairing code codec; byte-for-byte mirror of
 * `remote_connect/pairing.rs` (Crockford Base32, version 1 payload).
 *
 * Layout: 1 version + 32 psk + 16 offerer device id + 32 offerer public key
 * + 4 check bytes. The check is the first 4 bytes of SHA-256 over a domain
 * string and the payload prefix — enough to catch manual-entry typos.
 */

import { sha256 } from '@noble/hashes/sha2.js';

const VERSION = 1;
const CHECK_LEN = 4;
const DEVICE_ID_BYTES = 16;
const PAYLOAD_LEN = 1 + 32 + DEVICE_ID_BYTES + 32 + CHECK_LEN;
const ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';

export interface PskPairingPayload {
  psk: Uint8Array;
  offererDeviceId: string;
  offererPublicKey: Uint8Array;
}

function checkValue(prefix: Uint8Array): Uint8Array {
  const hasher = sha256.create();
  hasher.update(new TextEncoder().encode('OpenBitFun PSK pairing code v1'));
  hasher.update(prefix);
  return new Uint8Array(hasher.digest().slice(0, CHECK_LEN));
}

function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('');
}

function fromHex(value: string): Uint8Array | null {
  if (!/^[0-9a-fA-F]+$/.test(value) || value.length % 2 !== 0) return null;
  const out = new Uint8Array(value.length / 2);
  for (let i = 0; i < out.length; i += 1) out[i] = parseInt(value.slice(i * 2, i * 2 + 2), 16);
  return out;
}

function base32Encode(data: Uint8Array): string {
  let out = '';
  for (let offset = 0; offset < data.length; offset += 5) {
    let buf = 0n;
    for (let i = 0; i < 5; i += 1) buf = (buf << 8n) | BigInt(data[offset + i] ?? 0);
    for (let shift = 7; shift >= 0; shift -= 1) out += ALPHABET[Number((buf >> BigInt(shift * 5)) & 31n)];
  }
  return out;
}

function base32Decode(text: string): Uint8Array {
  const bits: number[] = [];
  for (const ch of text) {
    const upper = ch.toUpperCase();
    let value: number;
    if (upper === '0' || upper === 'O') value = 0;
    else if (upper === '1' || upper === 'I' || upper === 'L') value = 1;
    else {
      const index = ALPHABET.indexOf(upper);
      if (index === -1) throw new Error(`Invalid pairing code character: ${ch}`);
      value = index;
    }
    for (let shift = 4; shift >= 0; shift -= 1) bits.push((value >> shift) & 1);
  }
  const out = new Uint8Array(Math.floor(bits.length / 8));
  for (let i = 0; i < out.length; i += 1) {
    let byte = 0;
    for (let bit = 0; bit < 8; bit += 1) byte = (byte << 1) | bits[i * 8 + bit];
    out[i] = byte;
  }
  if (bits.length % 8 !== 0) throw new Error('Pairing code length is not a multiple of 8 characters.');
  return out;
}

/** A fresh random pairing secret (256-bit). */
export function generatePsk(): Uint8Array {
  const psk = new Uint8Array(32);
  (globalThis.crypto || (globalThis as { msCrypto?: Crypto }).msCrypto!).getRandomValues(psk);
  return psk;
}

export function encodePskPairingCode(payload: PskPairingPayload): string {
  const deviceId = fromHex(payload.offererDeviceId.trim());
  if (!deviceId || deviceId.length !== DEVICE_ID_BYTES) throw new Error('Invalid offerer device id.');
  const body = new Uint8Array(PAYLOAD_LEN - CHECK_LEN);
  body[0] = VERSION;
  body.set(payload.psk, 1);
  body.set(deviceId, 33);
  body.set(payload.offererPublicKey, 33 + DEVICE_ID_BYTES);
  const full = new Uint8Array(PAYLOAD_LEN);
  full.set(body);
  full.set(checkValue(body), PAYLOAD_LEN - CHECK_LEN);
  const encoded = base32Encode(full);
  return (encoded.match(/.{1,4}/g) ?? []).join('-');
}

export function parsePskPairingCode(code: string): PskPairingPayload {
  const compact = code.replace(/[-\s]/g, '');
  const data = base32Decode(compact);
  if (data.length !== PAYLOAD_LEN) throw new Error('Unexpected pairing code length.');
  const body = data.slice(0, PAYLOAD_LEN - CHECK_LEN);
  const check = data.slice(PAYLOAD_LEN - CHECK_LEN);
  if (body[0] !== VERSION) throw new Error(`Unsupported pairing code version: ${body[0]}.`);
  const expected = checkValue(body);
  for (let i = 0; i < CHECK_LEN; i += 1) {
    if (expected[i] !== check[i]) throw new Error('Pairing code check failed; please re-copy the code.');
  }
  const psk = body.slice(1, 33);
  const deviceId = toHex(body.slice(33, 33 + DEVICE_ID_BYTES));
  const publicKey = body.slice(33 + DEVICE_ID_BYTES);
  return { psk, offererDeviceId: deviceId, offererPublicKey: publicKey };
}
