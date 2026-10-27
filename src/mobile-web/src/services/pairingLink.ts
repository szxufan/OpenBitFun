export const OFFICIAL_RELAY_URL = 'https://remote.openbitfun.com/v/1.0.2';

export function normalizeRelayUrl(value: string): string | null {
  try {
    const url = new URL(value);
    if (!['http:', 'https:'].includes(url.protocol) || !url.hostname
      || url.username || url.password || url.search || url.hash) return null;
    return url.href.replace(/\/+$/, '');
  } catch { return null; }
}

function isLocalAddress(hostname: string): boolean {
  if (hostname === 'localhost' || hostname === '[::1]') return true;
  const octets = hostname.split('.').map(Number);
  if (octets.length === 4 && octets.every(value => Number.isInteger(value) && value >= 0 && value <= 255)) {
    return octets[0] === 10 || octets[0] === 127
      || (octets[0] === 192 && octets[1] === 168)
      || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31)
      || (octets[0] === 169 && octets[1] === 254);
  }
  return /^\[(?:f[cd][0-9a-f]{2}|fe[89ab][0-9a-f]):/i.test(hostname);
}

/** Invitations may select the official endpoint or a locally hosted Relay. */
export function pairingRelayUrl(value: string): string | null {
  const normalized = normalizeRelayUrl(value);
  if (!normalized) return null;
  if (normalized === OFFICIAL_RELAY_URL) return normalized;
  const url = new URL(normalized);
  return isLocalAddress(url.hostname) && url.pathname === '/' ? normalized : null;
}

export function currentRelayUrl(location: Pick<Location, 'origin' | 'pathname'> = window.location): string {
  const endpoint = pairingRelayUrl(`${location.origin}${location.pathname.replace(/index\.html$/, '')}`);
  if (!endpoint) throw new Error('Open this page from the official Relay or a local Relay invitation.');
  return endpoint;
}

/** A target is resolved only through the authenticated device directory. A
 * PSK pairing invitation may additionally carry the out-of-band pairing code
 * (`k`); it stays in the fragment and is validated as Crockford-style text. */
export function accountDeviceIdFromHash(hash: string): string | null {
  if (!hash.startsWith('#/pair?')) return null;
  const params = new URLSearchParams(hash.slice(7));
  const ids = params.getAll('did');
  const extraKeys = Array.from(params.keys()).filter(key => key !== 'did' && key !== 'k');
  if (extraKeys.length > 0 || ids.length !== 1
    || !/^[A-Za-z0-9_.-]{1,128}$/.test(ids[0]) || ['.', '..'].includes(ids[0])) return null;
  const code = params.get('k');
  if (code !== null && !/^[0-9A-Za-z-]{100,400}$/.test(code)) return null;
  return ids[0];
}

/** The out-of-band pairing code of a PSK pairing invitation, if present. */
export function pskPairingCodeFromHash(hash: string): string | null {
  if (!hash.startsWith('#/pair?')) return null;
  if (accountDeviceIdFromHash(hash) === null) return null;
  return new URLSearchParams(hash.slice(7)).get('k');
}
