import { describe, expect, it } from 'vitest';

import { createServerCertificateHashes, parseServerCertificateHash } from './hash';

const sampleHex = '00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff';
const sampleBase64 = 'ABEiM0RVZneImaq7zN3u/wARIjNEVWZ3iJmqu8zd7v8=';

describe('server certificate hash helpers', () => {
  it('parses hex input', () => {
    const bytes = parseServerCertificateHash(sampleHex);
    expect(bytes).toHaveLength(32);
    expect(bytes[0]).toBe(0x00);
    expect(bytes[31]).toBe(0xff);
  });

  it('parses colon-delimited hex input', () => {
    const bytes = parseServerCertificateHash('00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff');
    expect(bytes).toHaveLength(32);
    expect(Array.from(bytes)).toEqual(Array.from(parseServerCertificateHash(sampleHex)));
  });

  it('parses base64 input', () => {
    const bytes = parseServerCertificateHash(sampleBase64);
    expect(Array.from(bytes)).toEqual(Array.from(parseServerCertificateHash(sampleHex)));
  });

  it('creates a WebTransport-compatible hash structure', () => {
    const hashes = createServerCertificateHashes(sampleHex);
    expect(hashes).toHaveLength(1);
    expect(hashes?.[0].algorithm).toBe('sha-256');
    expect(Array.from(new Uint8Array(hashes?.[0].value ?? new ArrayBuffer(0)))).toEqual(
      Array.from(parseServerCertificateHash(sampleHex)),
    );
  });

  it('rejects malformed input', () => {
    expect(() => parseServerCertificateHash('not-a-hash')).toThrow(/SHA-256/);
  });

  it('treats empty input as omitted', () => {
    expect(createServerCertificateHashes('')).toBeUndefined();
  });
});
