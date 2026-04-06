// @vitest-environment jsdom

import { describe, it, expect } from 'vitest';
import {
  setConnectionState,
  updateStats,
  updateResolution,
  type UIElements,
} from './ui';

function createMockUI(): UIElements {
  return {
    root: document.createElement('div'),
    container: document.createElement('div'),
    canvas: document.createElement('canvas'),
    statusDot: document.createElement('span'),
    statusText: document.createElement('span'),
    stats: document.createElement('span'),
    resolutionDisplay: document.createElement('span'),
    connectScreen: document.createElement('div'),
    errorMsg: document.createElement('div'),
    connectBtn: document.createElement('button'),
    serverUrlInput: document.createElement('input'),
    certHashInput: document.createElement('textarea'),
    fullscreenBtn: document.createElement('button'),
    pointerLockBtn: document.createElement('button'),
    keyframeBtn: document.createElement('button'),
    statusBar: document.createElement('div'),
    toolbar: document.createElement('div'),
    coherenceBtn: document.createElement('button') as HTMLButtonElement,
    coherenceError: document.createElement('div'),
  };
}

describe('UI helpers', () => {
  describe('updateStats', () => {
    it('formats stats correctly', () => {
      const ui = createMockUI();
      updateStats(ui, 60, 1200, 4.567);
      expect(ui.stats.textContent).toBe('60 fps | 1200 frames | decode: 4.6ms');
    });

    it('handles zero values', () => {
      const ui = createMockUI();
      updateStats(ui, 0, 0, 0);
      expect(ui.stats.textContent).toBe('0 fps | 0 frames | decode: 0.0ms');
    });
  });

  describe('updateResolution', () => {
    it('formats resolution correctly', () => {
      const ui = createMockUI();
      updateResolution(ui, 1920, 1080);
      expect(ui.resolutionDisplay.textContent).toBe('1920x1080');
    });
  });

  describe('setConnectionState', () => {
    it('sets connected state', () => {
      const ui = createMockUI();
      setConnectionState(ui, 'connected');
      expect(ui.statusDot.className).toContain('connected');
      expect(ui.statusText.textContent).toBe('Connected');
      expect(ui.connectScreen.classList.contains('phantom-screen-hidden')).toBe(true);
    });

    it('sets error state with message', () => {
      const ui = createMockUI();
      setConnectionState(ui, 'error', 'Connection failed');
      expect(ui.statusDot.className).toContain('error');
      expect(ui.statusText.textContent).toBe('Connection failed');
      expect(ui.errorMsg.textContent).toBe('Connection failed');
      expect(ui.connectScreen.classList.contains('phantom-screen-hidden')).toBe(false);
    });

    it('sets connecting state and disables button', () => {
      const ui = createMockUI();
      setConnectionState(ui, 'connecting');
      expect(ui.connectBtn.disabled).toBe(true);
    });

    it('re-enables button on disconnect', () => {
      const ui = createMockUI();
      setConnectionState(ui, 'connecting');
      setConnectionState(ui, 'disconnected');
      expect(ui.connectBtn.disabled).toBe(false);
    });
  });
});
