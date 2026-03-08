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

describe('Input Protocol Serialization', () => {
  describe('encodeMouseMove', () => {
    it('should encode mouse move with correct format', () => {
      const buf = encodeMouseMove(1000, 500);
      expect(buf.length).toBe(5);
      expect(buf[0]).toBe(0x01);
      const view = new DataView(buf.buffer);
      expect(view.getUint16(1, false)).toBe(1000);
      expect(view.getUint16(3, false)).toBe(500);
    });

    it('should round fractional coordinates', () => {
      const buf = encodeMouseMove(100.7, 200.3);
      const view = new DataView(buf.buffer);
      expect(view.getUint16(1, false)).toBe(101);
      expect(view.getUint16(3, false)).toBe(200);
    });

    it('should handle zero coordinates', () => {
      const buf = encodeMouseMove(0, 0);
      const view = new DataView(buf.buffer);
      expect(view.getUint16(1, false)).toBe(0);
      expect(view.getUint16(3, false)).toBe(0);
    });
  });

  describe('encodeMouseButton', () => {
    it('should encode left button press', () => {
      const buf = encodeMouseButton(0, true);
      expect(buf.length).toBe(3);
      expect(buf[0]).toBe(0x02);
      expect(buf[1]).toBe(1); // DOM 0 -> X11 1 (left)
      expect(buf[2]).toBe(1);
    });

    it('should encode right button release', () => {
      const buf = encodeMouseButton(2, false);
      expect(buf[0]).toBe(0x02);
      expect(buf[1]).toBe(3); // DOM 2 -> X11 3 (right)
      expect(buf[2]).toBe(0);
    });

    it('should encode middle button', () => {
      const buf = encodeMouseButton(1, true);
      expect(buf[1]).toBe(2); // DOM 1 -> X11 2 (middle)
    });
  });

  describe('encodeMouseScroll', () => {
    it('should encode scroll with correct format', () => {
      const buf = encodeMouseScroll(0, 120); // scroll down 1 step
      expect(buf.length).toBe(5);
      expect(buf[0]).toBe(0x03);
      const view = new DataView(buf.buffer);
      expect(view.getInt16(1, false)).toBe(0); // dx
      expect(view.getInt16(3, false)).toBe(1); // dy (normalized)
    });

    it('should normalize large deltas', () => {
      const buf = encodeMouseScroll(0, -360); // 3 steps up
      const view = new DataView(buf.buffer);
      expect(view.getInt16(3, false)).toBe(-3);
    });
  });

  describe('encodeKeyEvent', () => {
    it('should encode key press', () => {
      const buf = encodeKeyEvent('KeyA', true);
      expect(buf[0]).toBe(0x10);
      expect(buf[1]).toBe(4); // "KeyA".length
      expect(new TextDecoder().decode(buf.slice(2, 6))).toBe('KeyA');
      expect(buf[6]).toBe(1); // pressed
    });

    it('should encode key release', () => {
      const buf = encodeKeyEvent('ShiftLeft', false);
      expect(buf[0]).toBe(0x10);
      expect(buf[1]).toBe(9); // "ShiftLeft".length
      expect(new TextDecoder().decode(buf.slice(2, 11))).toBe('ShiftLeft');
      expect(buf[11]).toBe(0); // released
    });

    it('should handle short key codes', () => {
      const buf = encodeKeyEvent('F1', true);
      expect(buf[1]).toBe(2);
      expect(new TextDecoder().decode(buf.slice(2, 4))).toBe('F1');
    });
  });

  describe('encodeClipboard', () => {
    it('should encode clipboard text', () => {
      const buf = encodeClipboard('Hello');
      expect(buf[0]).toBe(0x20);
      const view = new DataView(buf.buffer);
      expect(view.getUint32(1, false)).toBe(5); // text length
      expect(new TextDecoder().decode(buf.slice(5))).toBe('Hello');
    });

    it('should handle empty clipboard', () => {
      const buf = encodeClipboard('');
      expect(buf[0]).toBe(0x20);
      const view = new DataView(buf.buffer);
      expect(view.getUint32(1, false)).toBe(0);
      expect(buf.length).toBe(5);
    });

    it('should handle unicode text', () => {
      const text = 'Hello 世界';
      const buf = encodeClipboard(text);
      const encoder = new TextEncoder();
      const expected = encoder.encode(text);
      const view = new DataView(buf.buffer);
      expect(view.getUint32(1, false)).toBe(expected.length);
      expect(new TextDecoder().decode(buf.slice(5))).toBe(text);
    });
  });

  describe('Control messages', () => {
    it('should encode keyframe request', () => {
      const buf = encodeKeyframeRequest();
      expect(buf.length).toBe(2);
      expect(buf[0]).toBe(0x30);
      expect(buf[1]).toBe(0x01);
    });

    it('should encode bitrate request', () => {
      const buf = encodeBitrateRequest(6000);
      expect(buf.length).toBe(6);
      expect(buf[0]).toBe(0x30);
      expect(buf[1]).toBe(0x02);
      const view = new DataView(buf.buffer);
      expect(view.getUint32(2, false)).toBe(6000);
    });

    it('should encode resolution request', () => {
      const buf = encodeResolutionRequest(1920, 1080);
      expect(buf.length).toBe(6);
      expect(buf[0]).toBe(0x30);
      expect(buf[1]).toBe(0x03);
      const view = new DataView(buf.buffer);
      expect(view.getUint16(2, false)).toBe(1920);
      expect(view.getUint16(4, false)).toBe(1080);
    });
  });

  describe('Protocol symmetry with server', () => {
    it('mouse move bytes match server parser expectations', () => {
      // Server expects: [0x01] [x: u16 BE] [y: u16 BE]
      const buf = encodeMouseMove(1920, 1080);
      expect(buf[0]).toBe(0x01);
      expect(buf.length).toBe(5);
      // x = 1920 = 0x0780
      expect(buf[1]).toBe(0x07);
      expect(buf[2]).toBe(0x80);
      // y = 1080 = 0x0438
      expect(buf[3]).toBe(0x04);
      expect(buf[4]).toBe(0x38);
    });

    it('set resolution bytes match server parser expectations', () => {
      // Server expects: [0x30] [0x03] [w: u16 BE] [h: u16 BE]
      const buf = encodeResolutionRequest(1920, 1080);
      // Same raw bytes as the server test: [0x30, 0x03, 0x07, 0x80, 0x04, 0x38]
      expect(Array.from(buf)).toEqual([0x30, 0x03, 0x07, 0x80, 0x04, 0x38]);
    });

    it('set bitrate bytes match server parser expectations', () => {
      // Server expects: [0x30] [0x02] [kbps: u32 BE] = 6000 = 0x00001770
      const buf = encodeBitrateRequest(6000);
      expect(Array.from(buf)).toEqual([0x30, 0x02, 0x00, 0x00, 0x17, 0x70]);
    });
  });
});
