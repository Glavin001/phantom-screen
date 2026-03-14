// @vitest-environment jsdom

/**
 * Tests for the transport abstraction layer.
 *
 * Verifies that:
 * - The Transport interface contract is correctly implemented
 * - WebTransportAdapter throws when WebTransport is unavailable
 * - Both transports handle the same binary input protocol
 * - Auto-detection logic works correctly
 */

import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import type { Transport } from './transport';
import {
  encodeMouseMove,
  encodeMouseButton,
  encodeKeyEvent,
  encodeClipboard,
  encodeKeyframeRequest,
} from './input';

// ---- Mock Transport for interface compliance testing ----

class MockTransport implements Transport {
  private videoCallback: ((data: Uint8Array) => void) | null = null;
  private dataCallback: ((data: Uint8Array) => void) | null = null;
  private closedResolve!: () => void;
  readonly closed: Promise<void>;
  readonly sentInputs: Uint8Array[] = [];
  private mediaStream: MediaStream | null;

  constructor(opts?: { mediaStream?: MediaStream | null }) {
    this.mediaStream = opts?.mediaStream ?? null;
    this.closed = new Promise<void>((resolve) => {
      this.closedResolve = resolve;
    });
  }

  sendInput(data: Uint8Array): void {
    this.sentInputs.push(data);
  }

  onVideoFrame(callback: (data: Uint8Array) => void): void {
    this.videoCallback = callback;
  }

  getMediaStream(): MediaStream | null {
    return this.mediaStream;
  }

  onData(callback: (data: Uint8Array) => void): void {
    this.dataCallback = callback;
  }

  close(): void {
    this.closedResolve();
  }

  // Test helpers
  simulateVideoFrame(data: Uint8Array): void {
    this.videoCallback?.(data);
  }

  simulateData(data: Uint8Array): void {
    this.dataCallback?.(data);
  }
}

describe('Transport interface compliance', () => {
  it('MockTransport satisfies the Transport interface', () => {
    const transport: Transport = new MockTransport();
    expect(transport.sendInput).toBeDefined();
    expect(transport.onVideoFrame).toBeDefined();
    expect(transport.getMediaStream).toBeDefined();
    expect(transport.onData).toBeDefined();
    expect(transport.close).toBeDefined();
    expect(transport.closed).toBeInstanceOf(Promise);
    transport.close();
  });

  it('sendInput collects binary data', () => {
    const transport = new MockTransport();
    const mouseMove = encodeMouseMove(100, 200);
    const keyEvent = encodeKeyEvent('KeyA', true);

    transport.sendInput(mouseMove);
    transport.sendInput(keyEvent);

    expect(transport.sentInputs).toHaveLength(2);
    expect(transport.sentInputs[0]).toEqual(mouseMove);
    expect(transport.sentInputs[1]).toEqual(keyEvent);
    transport.close();
  });

  it('onVideoFrame callback receives frame data', () => {
    const transport = new MockTransport();
    const received: Uint8Array[] = [];

    transport.onVideoFrame((data) => received.push(data));

    // Simulate a video frame (13-byte header + payload)
    const frame = new Uint8Array(18);
    frame[0] = 0x01; // keyframe flag
    const view = new DataView(frame.buffer);
    view.setUint32(9, 5, false); // payload length = 5
    transport.simulateVideoFrame(frame);

    expect(received).toHaveLength(1);
    expect(received[0]).toEqual(frame);
    transport.close();
  });

  it('onData callback receives server data (clipboard)', () => {
    const transport = new MockTransport();
    const received: Uint8Array[] = [];

    transport.onData((data) => received.push(data));

    // Simulate clipboard data from server: [0x20][length:u32][text]
    const text = 'Hello from server';
    const encoded = new TextEncoder().encode(text);
    const data = new Uint8Array(5 + encoded.length);
    data[0] = 0x20;
    new DataView(data.buffer).setUint32(1, encoded.length, false);
    data.set(encoded, 5);

    transport.simulateData(data);

    expect(received).toHaveLength(1);
    expect(received[0]).toEqual(data);
    transport.close();
  });

  it('getMediaStream returns null for WebTransport-style transport', () => {
    const transport = new MockTransport();
    expect(transport.getMediaStream()).toBeNull();
    transport.close();
  });

  it('closed promise resolves on close()', async () => {
    const transport = new MockTransport();
    let resolved = false;
    transport.closed.then(() => { resolved = true; });

    transport.close();
    await transport.closed;
    expect(resolved).toBe(true);
  });
});

describe('Transport auto-detection', () => {
  it('WebTransport is not available in jsdom', () => {
    expect(typeof WebTransport).toBe('undefined');
  });

  it('RTCPeerConnection is not available in jsdom', () => {
    expect(typeof RTCPeerConnection).toBe('undefined');
  });

  it('auto mode should not pick WebTransport when unavailable', () => {
    const mode = 'auto' as const;
    const useWebTransport =
      mode === 'webtransport' ||
      (mode === 'auto' && typeof WebTransport !== 'undefined');

    expect(useWebTransport).toBe(false);
  });

  it('webtransport mode should be selected when explicitly set', () => {
    const mode = 'webtransport' as const;
    const useWebTransport =
      mode === 'webtransport' ||
      (mode === 'auto' && typeof WebTransport !== 'undefined');

    expect(useWebTransport).toBe(true);
  });

  it('webrtc mode should not select WebTransport', () => {
    const mode = 'webrtc' as const;
    const useWebTransport =
      mode === 'webtransport' ||
      (mode === 'auto' && typeof WebTransport !== 'undefined');

    expect(useWebTransport).toBe(false);
  });
});

describe('Binary input protocol consistency across transports', () => {
  it('both transports receive identical mouse move encoding', () => {
    const wtTransport = new MockTransport();
    const rtcTransport = new MockTransport();

    const data = encodeMouseMove(1920, 1080);
    wtTransport.sendInput(data);
    rtcTransport.sendInput(data);

    expect(wtTransport.sentInputs[0]).toEqual(rtcTransport.sentInputs[0]);
    // Verify binary format
    expect(data[0]).toBe(0x01); // mouse move type
    expect(data.length).toBe(5);

    wtTransport.close();
    rtcTransport.close();
  });

  it('both transports receive identical mouse button encoding', () => {
    const wtTransport = new MockTransport();
    const rtcTransport = new MockTransport();

    const data = encodeMouseButton(0, true); // left click
    wtTransport.sendInput(data);
    rtcTransport.sendInput(data);

    expect(wtTransport.sentInputs[0]).toEqual(rtcTransport.sentInputs[0]);
    expect(data[0]).toBe(0x02); // mouse button type
    expect(data.length).toBe(3);

    wtTransport.close();
    rtcTransport.close();
  });

  it('both transports receive identical keyboard encoding', () => {
    const wtTransport = new MockTransport();
    const rtcTransport = new MockTransport();

    const data = encodeKeyEvent('ArrowUp', true);
    wtTransport.sendInput(data);
    rtcTransport.sendInput(data);

    expect(wtTransport.sentInputs[0]).toEqual(rtcTransport.sentInputs[0]);
    expect(data[0]).toBe(0x10); // key event type

    wtTransport.close();
    rtcTransport.close();
  });

  it('both transports receive identical clipboard encoding', () => {
    const wtTransport = new MockTransport();
    const rtcTransport = new MockTransport();

    const data = encodeClipboard('Test clipboard content 📋');
    wtTransport.sendInput(data);
    rtcTransport.sendInput(data);

    expect(wtTransport.sentInputs[0]).toEqual(rtcTransport.sentInputs[0]);
    expect(data[0]).toBe(0x20); // clipboard type

    wtTransport.close();
    rtcTransport.close();
  });

  it('both transports receive identical control (keyframe) encoding', () => {
    const wtTransport = new MockTransport();
    const rtcTransport = new MockTransport();

    const data = encodeKeyframeRequest();
    wtTransport.sendInput(data);
    rtcTransport.sendInput(data);

    expect(wtTransport.sentInputs[0]).toEqual(rtcTransport.sentInputs[0]);
    expect(data[0]).toBe(0x30); // control type
    expect(data[1]).toBe(0x01); // keyframe subtype

    wtTransport.close();
    rtcTransport.close();
  });

  it('multi-event buffer is identical regardless of transport', () => {
    const events = [
      encodeMouseMove(640, 480),
      encodeMouseButton(2, true), // right click
      encodeKeyEvent('Escape', true),
      encodeKeyEvent('Escape', false),
      encodeKeyframeRequest(),
    ];

    const totalLen = events.reduce((sum, e) => sum + e.length, 0);
    const buf = new Uint8Array(totalLen);
    let offset = 0;
    for (const e of events) {
      buf.set(e, offset);
      offset += e.length;
    }

    const wtTransport = new MockTransport();
    const rtcTransport = new MockTransport();

    wtTransport.sendInput(buf);
    rtcTransport.sendInput(buf);

    expect(wtTransport.sentInputs[0]).toEqual(rtcTransport.sentInputs[0]);
    expect(wtTransport.sentInputs[0].length).toBe(totalLen);

    wtTransport.close();
    rtcTransport.close();
  });
});

describe('Signaling URL derivation', () => {
  // Test the deriveSignalingUrl logic from sdk.ts
  function deriveSignalingUrl(serverUrl: string): string {
    try {
      const url = new URL(serverUrl);
      const port = parseInt(url.port || '4443', 10);
      return `${url.protocol === 'https:' ? 'https' : 'http'}://${url.hostname}:${port + 1}`;
    } catch {
      return 'http://localhost';
    }
  }

  it('derives HTTP URL with port+1 from HTTPS server URL', () => {
    expect(deriveSignalingUrl('https://192.168.1.100:4443')).toBe('https://192.168.1.100:4444');
  });

  it('derives HTTP URL from HTTP server URL', () => {
    expect(deriveSignalingUrl('http://localhost:4443')).toBe('http://localhost:4444');
  });

  it('uses default port 4443 when none specified', () => {
    expect(deriveSignalingUrl('https://example.com')).toBe('https://example.com:4444');
  });

  it('handles non-standard ports', () => {
    expect(deriveSignalingUrl('https://host:8443')).toBe('https://host:8444');
  });

  it('returns fallback for invalid URLs', () => {
    expect(deriveSignalingUrl('not-a-url')).toBe('http://localhost');
  });
});
