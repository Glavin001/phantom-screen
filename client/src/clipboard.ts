/**
 * Clipboard synchronization between browser and remote desktop.
 */

export class ClipboardSync {
  private lastClipboardText = '';

  /** Handle incoming clipboard text from the server */
  async receiveClipboard(text: string): Promise<void> {
    if (text === this.lastClipboardText) return;
    this.lastClipboardText = text;

    try {
      await navigator.clipboard.writeText(text);
    } catch (e) {
      // Clipboard API requires user gesture or focus
      console.warn('Failed to write to clipboard (needs user focus):', e);
    }
  }

  /** Read current clipboard for sending to server */
  async readClipboard(): Promise<string | null> {
    try {
      const text = await navigator.clipboard.readText();
      if (text !== this.lastClipboardText) {
        this.lastClipboardText = text;
        return text;
      }
    } catch {
      // Clipboard read requires permission
    }
    return null;
  }
}
