// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { ControlManager } from './control';
import type { UIElements } from './ui';

function createMockUI(): UIElements {
  const canvas = document.createElement('canvas');
  return {
    root: document.createElement('div'),
    container: document.createElement('div'),
    canvas,
    statusDot: document.createElement('span'),
    statusText: document.createElement('span'),
    stats: document.createElement('span'),
    resolutionDisplay: document.createElement('span'),
    connectScreen: document.createElement('div'),
    errorMsg: document.createElement('div'),
    connectBtn: document.createElement('button'),
    serverUrlInput: document.createElement('input'),
    certHashInput: document.createElement('input'),
    fullscreenBtn: document.createElement('button'),
    pointerLockBtn: document.createElement('button'),
    keyframeBtn: document.createElement('button'),
    statusBar: document.createElement('div'),
    toolbar: document.createElement('div'),
  };
}

describe('ControlManager', () => {
  let send: ReturnType<typeof vi.fn>;
  let ui: UIElements;
  let ctrl: ControlManager;

  beforeEach(() => {
    vi.useFakeTimers();
    send = vi.fn();
    ui = createMockUI();
    ctrl = new ControlManager(send, ui);
  });

  afterEach(() => {
    ctrl.destroy();
    vi.useRealTimers();
  });

  describe('recordFrame / stats', () => {
    it('updates stats display after 1 second', () => {
      ctrl.recordFrame(5.0);
      ctrl.recordFrame(3.0);
      ctrl.recordFrame(7.0);

      vi.advanceTimersByTime(1000);

      // Stats should show fps, frame count, and average decode time
      expect(ui.stats.textContent).toContain('fps');
      expect(ui.stats.textContent).toContain('3 frames');
    });

    it('resets decode averages each stats interval', () => {
      ctrl.recordFrame(10.0);
      vi.advanceTimersByTime(1000);
      const firstStats = ui.stats.textContent;

      ctrl.recordFrame(2.0);
      vi.advanceTimersByTime(1000);
      const secondStats = ui.stats.textContent;

      // Second interval should only average the one 2ms frame, not include the 10ms
      expect(secondStats).toContain('2.0ms');
      expect(firstStats).toContain('10.0ms');
    });
  });

  describe('requestKeyframe', () => {
    it('sends a keyframe request message', () => {
      ctrl.requestKeyframe();
      expect(send).toHaveBeenCalledTimes(1);
      const buf = send.mock.calls[0][0] as Uint8Array;
      expect(buf[0]).toBe(0x30);
      expect(buf[1]).toBe(0x01);
    });

    it('sends keyframe on button click', () => {
      ui.keyframeBtn.click();
      expect(send).toHaveBeenCalledTimes(1);
      const buf = send.mock.calls[0][0] as Uint8Array;
      expect(buf[0]).toBe(0x30);
      expect(buf[1]).toBe(0x01);
    });
  });

  describe('setBitrate', () => {
    it('sends a bitrate request message', () => {
      ctrl.setBitrate(8000);
      expect(send).toHaveBeenCalledTimes(1);
      const buf = send.mock.calls[0][0] as Uint8Array;
      expect(buf[0]).toBe(0x30);
      expect(buf[1]).toBe(0x02);
      const view = new DataView(buf.buffer, buf.byteOffset);
      expect(view.getUint32(2, false)).toBe(8000);
    });
  });

  describe('setRemoteResolution', () => {
    it('updates canvas dimensions', () => {
      ctrl.setRemoteResolution(2560, 1440);
      expect(ui.canvas.width).toBe(2560);
      expect(ui.canvas.height).toBe(1440);
    });

    it('updates resolution display text', () => {
      ctrl.setRemoteResolution(1280, 720);
      expect(ui.resolutionDisplay.textContent).toBe('1280x720');
    });

    it('stores remote dimensions', () => {
      ctrl.setRemoteResolution(3840, 2160);
      expect(ctrl.getRemoteWidth()).toBe(3840);
      expect(ctrl.getRemoteHeight()).toBe(2160);
    });
  });

  describe('destroy', () => {
    it('stops updating stats after destroy', () => {
      ctrl.destroy();
      ctrl.recordFrame(5.0);
      vi.advanceTimersByTime(2000);
      // Stats should not have been updated (interval cleared)
      expect(ui.stats.textContent).toBe('');
    });

    it('removes keyframe button listener after destroy', () => {
      ctrl.destroy();
      ui.keyframeBtn.click();
      expect(send).not.toHaveBeenCalled();
    });
  });
});
