export interface ServerCertificateHash {
  algorithm: 'sha-256';
  value: ArrayBuffer;
}

function decodeBase64(value: string): Uint8Array {
  const normalized = value.replaceAll('-', '+').replaceAll('_', '/');
  const padding = normalized.length % 4 === 0 ? '' : '='.repeat(4 - (normalized.length % 4));
  const base64 = normalized + padding;

  if (typeof atob !== 'function') {
    throw new Error('Base64 decoding is unavailable in this runtime');
  }

  const binary = atob(base64);
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}

export function parseServerCertificateHash(value: string | Uint8Array): Uint8Array {
  if (value instanceof Uint8Array) {
    if (value.length !== 32) {
      throw new Error('Expected a 32-byte SHA-256 certificate hash');
    }
    return new Uint8Array(value);
  }

  const trimmed = value.trim();
  if (!trimmed) {
    throw new Error('Server certificate hash cannot be empty');
  }

  const withoutPrefix = trimmed.replace(/^sha-?256:/i, '');
  const maybeHex = withoutPrefix.replace(/[:\s-]/g, '');
  if (/^[0-9a-fA-F]{64}$/.test(maybeHex)) {
    const bytes = new Uint8Array(32);
    for (let index = 0; index < bytes.length; index += 1) {
      bytes[index] = Number.parseInt(maybeHex.slice(index * 2, index * 2 + 2), 16);
    }
    return bytes;
  }

  const bytes = decodeBase64(withoutPrefix);
  if (bytes.length !== 32) {
    throw new Error('Expected a 32-byte SHA-256 certificate hash in hex or base64');
  }
  return bytes;
}

export function createServerCertificateHashes(
  value?: string | Uint8Array | null,
): ServerCertificateHash[] | undefined {
  if (value == null) {
    return undefined;
  }

  const trimmed = typeof value === 'string' ? value.trim() : value;
  if (trimmed === '') {
    return undefined;
  }

  const bytes = parseServerCertificateHash(trimmed);
  const arrayBuffer = bytes.buffer.slice(
    bytes.byteOffset,
    bytes.byteOffset + bytes.byteLength,
  ) as ArrayBuffer;

  return [{ algorithm: 'sha-256', value: arrayBuffer }];
}
