// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { mountPhantomScreen } from './index';

const mockContext = {
  drawImage: vi.fn(),
} as unknown as CanvasRenderingContext2D;

describe('mountPhantomScreen', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(mockContext);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('renders inside a shadow root by default', () => {
    const root = document.createElement('div');
    document.body.append(root);

    const client = mountPhantomScreen(root, {
      serverUrl: 'https://demo.example:4443',
      serverCertificateHash: '00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff',
    });

    expect(root.shadowRoot).not.toBeNull();
    expect(
      (root.shadowRoot!.querySelector('[data-phantom-screen="server-url"]') as HTMLInputElement).value,
    ).toBe('https://demo.example:4443');
    expect(
      (root.shadowRoot!.querySelector('[data-phantom-screen="cert-hash"]') as HTMLInputElement).value,
    ).toContain('00112233');

    client.destroy();
  });

  it('supports rendering without shadow DOM', () => {
    const root = document.createElement('div');
    document.body.append(root);

    const client = mountPhantomScreen(root, {
      useShadowDom: false,
      serverUrl: 'https://127.0.0.1:4443',
    });

    expect(root.shadowRoot).toBeNull();
    expect(root.querySelector('[data-phantom-screen="connect-btn"]')).not.toBeNull();

    client.destroy();
  });

  it('shows a helpful error when WebTransport is forced but unavailable', async () => {
    const root = document.createElement('div');
    document.body.append(root);

    const client = mountPhantomScreen(root, {
      useShadowDom: false,
      serverUrl: 'https://127.0.0.1:4443',
      transport: 'webtransport',
    });

    await client.connect();

    expect(client.getState()).toBe('error');
    expect(root.textContent).toContain('WebTransport is not available in this browser');

    client.destroy();
  });

  it('falls back to WebRTC when WebTransport is unavailable in auto mode', async () => {
    const root = document.createElement('div');
    document.body.append(root);

    const client = mountPhantomScreen(root, {
      useShadowDom: false,
      serverUrl: 'https://127.0.0.1:4443',
    });

    await client.connect();

    // In jsdom, neither WebTransport nor RTCPeerConnection exists,
    // so the WebRTC fallback also fails.
    expect(client.getState()).toBe('error');
    expect(root.textContent).toContain('Failed to connect');

    client.destroy();
  });
});
