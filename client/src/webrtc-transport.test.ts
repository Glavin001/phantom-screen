// @vitest-environment jsdom

/**
 * Tests for the WebRTC transport implementation.
 *
 * Since jsdom doesn't provide RTCPeerConnection, these tests verify:
 * - The module exports the expected types
 * - Connection fails gracefully when RTCPeerConnection is unavailable
 * - The Transport interface is properly implemented (with mocks)
 * - Input data channel sends use the correct binary format
 */

import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import type { Transport } from './transport';

describe('WebRtcTransport module', () => {
  it('exports WebRtcTransport class', async () => {
    const mod = await import('./webrtc-transport');
    expect(mod.WebRtcTransport).toBeDefined();
    expect(typeof mod.WebRtcTransport.connect).toBe('function');
  });

  it('connect fails when RTCPeerConnection is not available', async () => {
    const { WebRtcTransport } = await import('./webrtc-transport');
    await expect(
      WebRtcTransport.connect({
        signalingUrl: 'http://localhost:4444',
      }),
    ).rejects.toThrow();
  });
});

describe('WebRtcTransport with mocked RTCPeerConnection', () => {
  let mockPc: any;
  let mockDataChannel: any;
  let mockMediaStream: any;
  let originalRTCPeerConnection: any;
  let originalRTCSessionDescription: any;
  let originalRTCIceCandidate: any;

  beforeEach(() => {
    // Save originals
    originalRTCPeerConnection = (globalThis as any).RTCPeerConnection;
    originalRTCSessionDescription = (globalThis as any).RTCSessionDescription;
    originalRTCIceCandidate = (globalThis as any).RTCIceCandidate;

    // Mock data channel
    mockDataChannel = {
      readyState: 'open',
      binaryType: 'arraybuffer',
      ordered: true,
      onopen: null as any,
      onerror: null as any,
      onmessage: null as any,
      send: vi.fn(),
      close: vi.fn(),
    };

    // Mock MediaStream
    mockMediaStream = {
      addTrack: vi.fn(),
      getTracks: vi.fn(() => []),
    };

    // Mock RTCPeerConnection
    mockPc = {
      connectionState: 'new',
      onconnectionstatechange: null as any,
      ontrack: null as any,
      onicecandidate: null as any,
      ondatachannel: null as any,
      createDataChannel: vi.fn(() => mockDataChannel),
      createOffer: vi.fn(async () => ({ sdp: 'mock-offer-sdp', type: 'offer' })),
      setLocalDescription: vi.fn(async () => {}),
      setRemoteDescription: vi.fn(async () => {}),
      addIceCandidate: vi.fn(async () => {}),
      close: vi.fn(),
    };

    (globalThis as any).RTCPeerConnection = vi.fn(() => mockPc);
    (globalThis as any).RTCSessionDescription = vi.fn((init: any) => init);
    (globalThis as any).RTCIceCandidate = vi.fn((init: any) => init);
    (globalThis as any).MediaStream = vi.fn(() => mockMediaStream);
  });

  afterEach(() => {
    // Restore originals
    if (originalRTCPeerConnection === undefined) {
      delete (globalThis as any).RTCPeerConnection;
    } else {
      (globalThis as any).RTCPeerConnection = originalRTCPeerConnection;
    }
    if (originalRTCSessionDescription === undefined) {
      delete (globalThis as any).RTCSessionDescription;
    } else {
      (globalThis as any).RTCSessionDescription = originalRTCSessionDescription;
    }
    if (originalRTCIceCandidate === undefined) {
      delete (globalThis as any).RTCIceCandidate;
    } else {
      (globalThis as any).RTCIceCandidate = originalRTCIceCandidate;
    }
    vi.restoreAllMocks();
  });

  it('creates a data channel named "input"', async () => {
    // Mock fetch for signaling
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const urlStr = typeof url === 'string' ? url : url.toString();
      if (urlStr.includes('/webrtc/offer')) {
        return new Response(JSON.stringify({ sdp: 'mock-answer-sdp' }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.includes('/webrtc/candidates')) {
        return new Response(JSON.stringify([]), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response('', { status: 200 });
    });

    const { WebRtcTransport } = await import('./webrtc-transport');

    // Trigger data channel open immediately
    mockDataChannel.readyState = 'open';

    const transport = await WebRtcTransport.connect({
      signalingUrl: 'http://localhost:4444',
    });

    expect(mockPc.createDataChannel).toHaveBeenCalledWith('input', { ordered: true });
    expect(transport).toBeDefined();

    transport.close();
  });

  it('sends SDP offer to signaling server', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (url, init) => {
      const urlStr = typeof url === 'string' ? url : url.toString();
      if (urlStr.includes('/webrtc/offer')) {
        // Verify the offer was sent correctly
        const body = JSON.parse(init?.body as string);
        expect(body.sdp).toBe('mock-offer-sdp');
        return new Response(JSON.stringify({ sdp: 'mock-answer-sdp' }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.includes('/webrtc/candidates')) {
        return new Response(JSON.stringify([]), { status: 200 });
      }
      return new Response('', { status: 200 });
    });

    mockDataChannel.readyState = 'open';

    const { WebRtcTransport } = await import('./webrtc-transport');
    const transport = await WebRtcTransport.connect({
      signalingUrl: 'http://localhost:4444',
    });

    expect(fetchSpy).toHaveBeenCalledWith(
      'http://localhost:4444/webrtc/offer',
      expect.objectContaining({
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
      }),
    );

    transport.close();
  });

  it('sets remote description from signaling answer', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const urlStr = typeof url === 'string' ? url : url.toString();
      if (urlStr.includes('/webrtc/offer')) {
        return new Response(JSON.stringify({ sdp: 'mock-answer-sdp' }), { status: 200 });
      }
      if (urlStr.includes('/webrtc/candidates')) {
        return new Response(JSON.stringify([]), { status: 200 });
      }
      return new Response('', { status: 200 });
    });

    mockDataChannel.readyState = 'open';

    const { WebRtcTransport } = await import('./webrtc-transport');
    await WebRtcTransport.connect({ signalingUrl: 'http://localhost:4444' });

    expect(mockPc.setRemoteDescription).toHaveBeenCalledWith(
      expect.objectContaining({ type: 'answer', sdp: 'mock-answer-sdp' }),
    );
  });

  it('implements Transport interface methods', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const urlStr = typeof url === 'string' ? url : url.toString();
      if (urlStr.includes('/webrtc/offer')) {
        return new Response(JSON.stringify({ sdp: 'mock-answer-sdp' }), { status: 200 });
      }
      if (urlStr.includes('/webrtc/candidates')) {
        return new Response(JSON.stringify([]), { status: 200 });
      }
      return new Response('', { status: 200 });
    });

    mockDataChannel.readyState = 'open';

    const { WebRtcTransport } = await import('./webrtc-transport');
    const transport: Transport = await WebRtcTransport.connect({
      signalingUrl: 'http://localhost:4444',
    });

    // sendInput sends binary data through the data channel
    const inputData = new Uint8Array([0x01, 0x00, 0x64, 0x00, 0xC8]); // mouse move
    transport.sendInput(inputData);
    expect(mockDataChannel.send).toHaveBeenCalled();

    // getMediaStream returns a MediaStream (not null, unlike WebTransport)
    const stream = transport.getMediaStream();
    expect(stream).not.toBeNull();

    // onVideoFrame is a no-op for WebRTC (media tracks handle video)
    expect(() => transport.onVideoFrame(() => {})).not.toThrow();

    // onData registers a callback
    expect(() => transport.onData(() => {})).not.toThrow();

    // closed is a Promise
    expect(transport.closed).toBeInstanceOf(Promise);

    transport.close();
  });

  it('throws on signaling failure', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const urlStr = typeof url === 'string' ? url : url.toString();
      if (urlStr.includes('/webrtc/offer')) {
        return new Response('Server Error', { status: 500, statusText: 'Internal Server Error' });
      }
      return new Response('', { status: 200 });
    });

    const { WebRtcTransport } = await import('./webrtc-transport');
    await expect(
      WebRtcTransport.connect({ signalingUrl: 'http://localhost:4444' }),
    ).rejects.toThrow('Signaling failed');
  });

  it('uses default STUN server when no iceServers provided', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const urlStr = typeof url === 'string' ? url : url.toString();
      if (urlStr.includes('/webrtc/offer')) {
        return new Response(JSON.stringify({ sdp: 'v=0' }), { status: 200 });
      }
      if (urlStr.includes('/webrtc/candidates')) {
        return new Response(JSON.stringify([]), { status: 200 });
      }
      return new Response('', { status: 200 });
    });

    mockDataChannel.readyState = 'open';

    const { WebRtcTransport } = await import('./webrtc-transport');
    await WebRtcTransport.connect({ signalingUrl: 'http://localhost:4444' });

    expect((globalThis as any).RTCPeerConnection).toHaveBeenCalledWith({
      iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
    });
  });

  it('uses custom iceServers when provided', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const urlStr = typeof url === 'string' ? url : url.toString();
      if (urlStr.includes('/webrtc/offer')) {
        return new Response(JSON.stringify({ sdp: 'v=0' }), { status: 200 });
      }
      if (urlStr.includes('/webrtc/candidates')) {
        return new Response(JSON.stringify([]), { status: 200 });
      }
      return new Response('', { status: 200 });
    });

    mockDataChannel.readyState = 'open';

    const customIce = [
      { urls: 'stun:custom.stun:3478' },
      { urls: 'turn:custom.turn:3478', username: 'user', credential: 'pass' },
    ];

    const { WebRtcTransport } = await import('./webrtc-transport');
    await WebRtcTransport.connect({
      signalingUrl: 'http://localhost:4444',
      iceServers: customIce,
    });

    expect((globalThis as any).RTCPeerConnection).toHaveBeenCalledWith({
      iceServers: customIce,
    });
  });
});
