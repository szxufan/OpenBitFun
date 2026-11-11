import { describe, expect, it } from 'vitest';
import {
  OFFICIAL_RELAY_URL,
  relayEndpointBase,
  relayEndpointVersion,
  relayPageUrl,
  sameRelayDeployment,
} from '../../../../shared/relay-transport/RelayEndpoint';

describe('relay endpoint identity', () => {
  it('serves the fork relay at the origin root without a version query', () => {
    expect(OFFICIAL_RELAY_URL).toBe('https://obf.test.linkerpi.com');
    expect(relayEndpointVersion(OFFICIAL_RELAY_URL)).toBeNull();
    expect(relayEndpointVersion(`${OFFICIAL_RELAY_URL}?v=1.0.10`)).toBe('1.0.10');
  });

  it('strips the version so paths are never appended to a query', () => {
    expect(relayEndpointBase(OFFICIAL_RELAY_URL)).toBe('https://obf.test.linkerpi.com');
    expect(relayEndpointBase(`${OFFICIAL_RELAY_URL}/?v=1.0.9`)).toBe(
      'https://obf.test.linkerpi.com',
    );
    expect(relayEndpointBase('https://relay.example.com/v/1.0.2/')).toBe(
      'https://relay.example.com/v/1.0.2',
    );
  });

  it('rejects credentials, fragments, and unknown query content', () => {
    for (const invalid of [
      `https://user@${OFFICIAL_RELAY_URL.replace('https://', '')}`,
      `${OFFICIAL_RELAY_URL}/#/pair`,
      `${OFFICIAL_RELAY_URL}?v=1.0.9&x=1`,
      `${OFFICIAL_RELAY_URL}?next=https://evil.example`,
      `ftp://${OFFICIAL_RELAY_URL.replace('https://', '')}`,
      '',
    ]) {
      expect(relayEndpointBase(invalid)).toBeNull();
    }
  });

  it('treats the origin-root deployment identity as stable and other releases as different', () => {
    expect(sameRelayDeployment(OFFICIAL_RELAY_URL, `${OFFICIAL_RELAY_URL}/`)).toBe(true);
    expect(sameRelayDeployment(OFFICIAL_RELAY_URL, `${OFFICIAL_RELAY_URL}?v=1.0.10`)).toBe(true);
    expect(sameRelayDeployment(OFFICIAL_RELAY_URL, `${OFFICIAL_RELAY_URL}/v/1.0.2`)).toBe(false);
    expect(sameRelayDeployment(OFFICIAL_RELAY_URL, 'https://other.example.com/relay')).toBe(false);
    expect(sameRelayDeployment(`${OFFICIAL_RELAY_URL.replace('https', 'http')}`, OFFICIAL_RELAY_URL)).toBe(false);
  });

  it('keeps the version before the fragment in user-facing links when present', () => {
    expect(relayPageUrl(OFFICIAL_RELAY_URL, '#/pair?did=abc')).toBe(
      'https://obf.test.linkerpi.com/#/pair?did=abc',
    );
    expect(relayPageUrl(`${OFFICIAL_RELAY_URL}?v=1.0.2`, '#/pair?did=abc')).toBe(
      'https://obf.test.linkerpi.com/?v=1.0.2#/pair?did=abc',
    );
  });
});
