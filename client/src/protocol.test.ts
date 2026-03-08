/**
 * Protocol integration tests — verify the full client/server binary protocol
 * works end-to-end by simulating server-side parsing of client-encoded messages,
 * and client-side parsing of server-encoded video frame headers.
 */
import { describe, it, expect } from 'vitest';
import {
  encodeMouseMove,
  encodeMouseButton,
  encodeMouseScroll,
  encodeKeyEvent,
  encodeClipboard,
  encodeKeyframeRequest,
  encodeBitrateRequest,
  encodeResolutionRequest,
} from './input';

// ---- Server-side parsing replicated in TypeScript for cross-validation ----

/** Mirrors server's estimate_event_length (input.rs) */
function estimateEventLength(data: Uint8Array): number {
  if (data.length === 0) return 0;
  switch (data[0]) {
    case 0x01: return 5;
    case 0x02: return 3;
    case 0x03: return 5;
    case 0x10: {
      if (data.length < 2) return 0;
      const codeLen = data[1];
      return 2 + codeLen + 1;
    }
    case 0x20: {
      if (data.length < 5) return 0;
      const view = new DataView(data.buffer, data.byteOffset);
      const length = view.getUint32(1, false);
      return 5 + length;
    }
    case 0x30: {
      if (data.length < 2) return 0;
      switch (data[1]) {
        case 0x01: return 2;
        case 0x02: return 6;
        case 0x03: return 6;
        default: return 0;
      }
    }
    default: return 0;
  }
}

type ParsedEvent =
  | { type: 'mouse_move'; x: number; y: number }
  | { type: 'mouse_button'; button: number; pressed: boolean }
  | { type: 'mouse_scroll'; dx: number; dy: number }
  | { type: 'key_event'; code: string; pressed: boolean }
  | { type: 'clipboard'; text: string }
  | { type: 'keyframe_request' }
  | { type: 'set_bitrate'; kbps: number }
  | { type: 'set_resolution'; width: number; height: number };

/** Mirrors server's parse_input_event (input.rs) */
function parseInputEvent(data: Uint8Array): ParsedEvent | null {
  if (data.length === 0) return null;
  const view = new DataView(data.buffer, data.byteOffset);

  switch (data[0]) {
    case 0x01:
      if (data.length < 5) return null;
      return { type: 'mouse_move', x: view.getUint16(1, false), y: view.getUint16(3, false) };
    case 0x02:
      if (data.length < 3) return null;
      return { type: 'mouse_button', button: data[1], pressed: data[2] !== 0 };
    case 0x03:
      if (data.length < 5) return null;
      return { type: 'mouse_scroll', dx: view.getInt16(1, false), dy: view.getInt16(3, false) };
    case 0x10: {
      if (data.length < 3) return null;
      const codeLen = data[1];
      if (data.length < 2 + codeLen + 1) return null;
      const code = new TextDecoder().decode(data.slice(2, 2 + codeLen));
      const pressed = data[2 + codeLen] !== 0;
      return { type: 'key_event', code, pressed };
    }
    case 0x20: {
      if (data.length < 5) return null;
      const length = view.getUint32(1, false);
      if (data.length < 5 + length) return null;
      const text = new TextDecoder().decode(data.slice(5, 5 + length));
      return { type: 'clipboard', text };
    }
    case 0x30: {
      if (data.length < 2) return null;
      switch (data[1]) {
        case 0x01: return { type: 'keyframe_request' };
        case 0x02:
          if (data.length < 6) return null;
          return { type: 'set_bitrate', kbps: view.getUint32(2, false) };
        case 0x03:
          if (data.length < 6) return null;
          return { type: 'set_resolution', width: view.getUint16(2, false), height: view.getUint16(4, false) };
        default: return null;
      }
    }
    default: return null;
  }
}

/** Mirrors server's video frame header format */
interface FrameHeader {
  isKeyframe: boolean;
  pts: bigint;
  dataLength: number;
}

function parseFrameHeader(header: Uint8Array): FrameHeader | null {
  if (header.length < 13) return null;
  const view = new DataView(header.buffer, header.byteOffset);
  return {
    isKeyframe: (header[0] & 0x01) !== 0,
    pts: view.getBigUint64(1, false),
    dataLength: view.getUint32(9, false),
  };
}

function encodeFrameHeader(isKeyframe: boolean, pts: bigint, dataLength: number): Uint8Array {
  const header = new Uint8Array(13);
  const view = new DataView(header.buffer);
  header[0] = isKeyframe ? 0x01 : 0x00;
  view.setBigUint64(1, pts, false);
  view.setUint32(9, dataLength, false);
  return header;
}

// ---- Tests ----

describe('Protocol Integration: Client encode → Server parse', () => {
  it('mouse move roundtrips correctly', () => {
    const encoded = encodeMouseMove(1920, 1080);
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'mouse_move', x: 1920, y: 1080 });
  });

  it('mouse button left press roundtrips correctly', () => {
    const encoded = encodeMouseButton(0, true); // DOM 0 = left → X11 1
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'mouse_button', button: 1, pressed: true });
  });

  it('mouse scroll roundtrips correctly', () => {
    const encoded = encodeMouseScroll(0, 120); // 1 step down
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'mouse_scroll', dx: 0, dy: 1 });
  });

  it('key event roundtrips correctly', () => {
    const encoded = encodeKeyEvent('ArrowDown', true);
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'key_event', code: 'ArrowDown', pressed: true });
  });

  it('clipboard text roundtrips correctly', () => {
    const text = 'Hello, World! 🌍';
    const encoded = encodeClipboard(text);
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'clipboard', text });
  });

  it('keyframe request roundtrips correctly', () => {
    const encoded = encodeKeyframeRequest();
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'keyframe_request' });
  });

  it('bitrate request roundtrips correctly', () => {
    const encoded = encodeBitrateRequest(8000);
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'set_bitrate', kbps: 8000 });
  });

  it('resolution request roundtrips correctly', () => {
    const encoded = encodeResolutionRequest(2560, 1440);
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'set_resolution', width: 2560, height: 1440 });
  });
});

describe('Protocol Integration: Multi-event buffer', () => {
  it('can parse multiple concatenated events', () => {
    // Simulate multiple events being sent in one chunk (server's process_input_data)
    const events: Uint8Array[] = [
      encodeMouseMove(100, 200),
      encodeMouseButton(0, true),
      encodeKeyEvent('KeyA', true),
      encodeKeyEvent('KeyA', false),
      encodeMouseButton(0, false),
    ];

    // Concatenate into single buffer
    const totalLen = events.reduce((sum, e) => sum + e.length, 0);
    const buf = new Uint8Array(totalLen);
    let offset = 0;
    for (const e of events) {
      buf.set(e, offset);
      offset += e.length;
    }

    // Parse all events from concatenated buffer
    const parsed: ParsedEvent[] = [];
    let parseOffset = 0;
    while (parseOffset < buf.length) {
      const remaining = buf.slice(parseOffset);
      const eventLen = estimateEventLength(remaining);
      expect(eventLen).toBeGreaterThan(0);

      const event = parseInputEvent(remaining.slice(0, eventLen));
      expect(event).not.toBeNull();
      parsed.push(event!);
      parseOffset += eventLen;
    }

    expect(parsed).toHaveLength(5);
    expect(parsed[0]).toEqual({ type: 'mouse_move', x: 100, y: 200 });
    expect(parsed[1]).toEqual({ type: 'mouse_button', button: 1, pressed: true });
    expect(parsed[2]).toEqual({ type: 'key_event', code: 'KeyA', pressed: true });
    expect(parsed[3]).toEqual({ type: 'key_event', code: 'KeyA', pressed: false });
    expect(parsed[4]).toEqual({ type: 'mouse_button', button: 1, pressed: false });
  });
});

describe('Protocol Integration: Video frame header', () => {
  it('encodes and parses keyframe header', () => {
    const pts = BigInt(1_000_000_000); // 1 second in nanoseconds
    const dataLen = 65536;
    const header = encodeFrameHeader(true, pts, dataLen);
    const parsed = parseFrameHeader(header);

    expect(parsed).not.toBeNull();
    expect(parsed!.isKeyframe).toBe(true);
    expect(parsed!.pts).toBe(pts);
    expect(parsed!.dataLength).toBe(dataLen);
  });

  it('encodes and parses non-keyframe header', () => {
    const pts = BigInt(33_333_333); // ~33ms (30fps)
    const dataLen = 4096;
    const header = encodeFrameHeader(false, pts, dataLen);
    const parsed = parseFrameHeader(header);

    expect(parsed).not.toBeNull();
    expect(parsed!.isKeyframe).toBe(false);
    expect(parsed!.pts).toBe(pts);
    expect(parsed!.dataLength).toBe(dataLen);
  });

  it('frame header is exactly 13 bytes', () => {
    const header = encodeFrameHeader(true, BigInt(0), 0);
    expect(header.length).toBe(13);
  });

  it('rejects truncated frame header', () => {
    const header = new Uint8Array(12); // 1 byte short
    expect(parseFrameHeader(header)).toBeNull();
  });
});

describe('Protocol Integration: Clipboard deduplication', () => {
  it('ClipboardSync deduplicates same text', async () => {
    // Simulate the deduplication logic from clipboard.ts
    let lastClipboardText = '';
    const received: string[] = [];

    function receiveClipboard(text: string) {
      if (text === lastClipboardText) return;
      lastClipboardText = text;
      received.push(text);
    }

    receiveClipboard('first');
    receiveClipboard('first');  // duplicate
    receiveClipboard('second');
    receiveClipboard('second'); // duplicate
    receiveClipboard('first');  // different from last

    expect(received).toEqual(['first', 'second', 'first']);
  });
});

describe('Protocol Integration: Edge cases', () => {
  it('handles maximum coordinate values', () => {
    const encoded = encodeMouseMove(65535, 65535);
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'mouse_move', x: 65535, y: 65535 });
  });

  it('handles maximum bitrate', () => {
    const encoded = encodeBitrateRequest(4294967295); // u32 max
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'set_bitrate', kbps: 4294967295 });
  });

  it('handles long key codes', () => {
    const code = 'NumpadSubtract'; // 14 chars
    const encoded = encodeKeyEvent(code, false);
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'key_event', code, pressed: false });
  });

  it('handles empty clipboard', () => {
    const encoded = encodeClipboard('');
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'clipboard', text: '' });
  });

  it('handles large clipboard text', () => {
    const text = 'A'.repeat(10000);
    const encoded = encodeClipboard(text);
    const parsed = parseInputEvent(encoded);
    expect(parsed).toEqual({ type: 'clipboard', text });
  });

  it('all mouse buttons map correctly for roundtrip', () => {
    // DOM → X11 button mapping: 0→1, 1→2, 2→3, 3→8, 4→9
    const expectedX11 = [1, 2, 3, 8, 9];
    for (let dom = 0; dom < 5; dom++) {
      const encoded = encodeMouseButton(dom, true);
      const parsed = parseInputEvent(encoded);
      expect(parsed).toEqual({
        type: 'mouse_button',
        button: expectedX11[dom],
        pressed: true,
      });
    }
  });
});
