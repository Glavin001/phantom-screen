// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { ClipboardSync } from './clipboard';

describe('ClipboardSync', () => {
  let clipboard: ClipboardSync;

  beforeEach(() => {
    clipboard = new ClipboardSync();
    // Mock the clipboard API
    Object.assign(navigator, {
      clipboard: {
        writeText: vi.fn().mockResolvedValue(undefined),
        readText: vi.fn().mockResolvedValue(''),
      },
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('receiveClipboard', () => {
    it('writes new text to the clipboard API', async () => {
      await clipboard.receiveClipboard('hello');
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith('hello');
    });

    it('deduplicates consecutive identical text', async () => {
      await clipboard.receiveClipboard('hello');
      await clipboard.receiveClipboard('hello');
      expect(navigator.clipboard.writeText).toHaveBeenCalledTimes(1);
    });

    it('writes again when text changes', async () => {
      await clipboard.receiveClipboard('first');
      await clipboard.receiveClipboard('second');
      expect(navigator.clipboard.writeText).toHaveBeenCalledTimes(2);
      expect(navigator.clipboard.writeText).toHaveBeenLastCalledWith('second');
    });

    it('writes again after text changes back', async () => {
      await clipboard.receiveClipboard('A');
      await clipboard.receiveClipboard('B');
      await clipboard.receiveClipboard('A');
      expect(navigator.clipboard.writeText).toHaveBeenCalledTimes(3);
    });

    it('does not throw when clipboard API fails', async () => {
      vi.mocked(navigator.clipboard.writeText).mockRejectedValueOnce(
        new Error('Document is not focused'),
      );
      // Should not throw
      await clipboard.receiveClipboard('text');
    });
  });

  describe('readClipboard', () => {
    it('returns new text from clipboard', async () => {
      vi.mocked(navigator.clipboard.readText).mockResolvedValue('from browser');
      const result = await clipboard.readClipboard();
      expect(result).toBe('from browser');
    });

    it('returns null when clipboard text matches last sent', async () => {
      await clipboard.receiveClipboard('same');
      vi.mocked(navigator.clipboard.readText).mockResolvedValue('same');
      const result = await clipboard.readClipboard();
      expect(result).toBeNull();
    });

    it('returns null when clipboard read fails', async () => {
      vi.mocked(navigator.clipboard.readText).mockRejectedValue(
        new Error('Permission denied'),
      );
      const result = await clipboard.readClipboard();
      expect(result).toBeNull();
    });

    it('returns new text after clipboard changes', async () => {
      vi.mocked(navigator.clipboard.readText).mockResolvedValue('first');
      expect(await clipboard.readClipboard()).toBe('first');

      // Same text - should return null
      vi.mocked(navigator.clipboard.readText).mockResolvedValue('first');
      expect(await clipboard.readClipboard()).toBeNull();

      // Changed - should return new value
      vi.mocked(navigator.clipboard.readText).mockResolvedValue('second');
      expect(await clipboard.readClipboard()).toBe('second');
    });
  });
});
