/**
 * Coherence Mode controller — orchestrates per-window browser popups
 * that each display a cropped video stream from a single X11 window.
 */

import { WindowPopup } from './coherence-popup';
import {
  encodeEnableCoherence,
  encodeDisableCoherence,
  encodeSubscribeWindow,
  encodeUnsubscribeWindow,
  encodeLaunchApp,
  type InputSender,
} from './input';

export interface WindowInfo {
  windowId: number;
  title: string;
  x: number;
  y: number;
  width: number;
  height: number;
  visible: boolean;
  appClass: string;
}

export type CoherenceEventHandler = {
  onWindowListChanged?: (windows: WindowInfo[]) => void;
  /** User-visible error for coherence pop-out / stream failures */
  onStreamError?: (message: string) => void;
};

export class CoherenceController {
  private windows = new Map<number, WindowInfo>();
  private popups = new Map<number, WindowPopup>();
  private send: InputSender;
  private handlers: CoherenceEventHandler;
  private active = false;
  private decoderAcceleration: VideoDecoderConfig['hardwareAcceleration'];
  private inlineParent: HTMLElement | null = null;

  constructor(
    send: InputSender,
    handlers: CoherenceEventHandler = {},
    decoderAcceleration: VideoDecoderConfig['hardwareAcceleration'] = 'prefer-software',
  ) {
    this.send = send;
    this.handlers = handlers;
    this.decoderAcceleration = decoderAcceleration;
  }

  /** Set the container element for inline window rendering */
  setInlineParent(el: HTMLElement): void {
    this.inlineParent = el;
  }

  /** Enable coherence mode — sends the protocol message to the server */
  enableCoherenceMode(): void {
    if (this.active) return;
    this.active = true;
    this.send(encodeEnableCoherence());
  }

  /** Disable coherence mode — closes all popups and sends disable message */
  disableCoherenceMode(): void {
    if (!this.active) return;
    this.active = false;
    this.send(encodeDisableCoherence());
    this.closeAllPopups();
    this.windows.clear();
    this.handlers.onWindowListChanged?.([]);
  }

  isActive(): boolean {
    return this.active;
  }

  getWindows(): WindowInfo[] {
    return Array.from(this.windows.values());
  }

  /** Open a window stream inline within the coherence panel */
  openWindowPopup(windowId: number): void {
    this.openWindow(windowId, true);
  }

  /** Open a window stream in a separate browser popup */
  openWindowAsPopup(windowId: number): void {
    this.openWindow(windowId, false);
  }

  private openWindow(windowId: number, inline: boolean): void {
    const info = this.windows.get(windowId);
    if (!info) return;

    const mode = inline ? 'inline' : 'popup';
    console.log(`[coherence] openWindow wid=${windowId} mode=${mode}`);
    this.handlers.onStreamError?.('');

    // Close existing view if already open (e.g., switching from inline to popup)
    const existing = this.popups.get(windowId);
    if (existing) {
      this.send(encodeUnsubscribeWindow(windowId));
      existing.close();
      this.popups.delete(windowId);
    }

    // Use `let` + assignment so callbacks can close over `popup` while the constructor
    // may invoke onStreamError synchronously (would hit TDZ with `const popup = new ...`).
    let popup: WindowPopup;
    popup = new WindowPopup(
      info,
      this.send,
      this.decoderAcceleration,
      inline ? (this.inlineParent ?? undefined) : undefined,
      () => {
        // Called when inline close button or popup beforeunload fires.
        // Guard: only delete from map if this is still the active popup for this window.
        // When switching modes (e.g. inline→popup), the old popup's beforeunload
        // can fire asynchronously AFTER the new popup is already in the map.
        if (this.popups.get(windowId) !== popup) {
          // Stale popup — ignore (old popup's beforeunload fired after new popup was created)
          return;
        }
        console.log(`[coherence] wid=${windowId} closed`);
        this.popups.delete(windowId);
        this.handlers.onWindowListChanged?.(this.getWindows());
      },
      () => {
        // Called when decoder needs a fresh keyframe (error recovery).
        // Guard: only act if this popup is still the active one.
        if (this.popups.get(windowId) !== popup) return;
        console.warn(`[coherence] wid=${windowId} requesting keyframe, re-subscribing`);
        this.send(encodeSubscribeWindow(windowId));
      },
      (msg) => {
        if (this.popups.get(windowId) !== popup) return;
        if (msg) this.handlers.onStreamError?.(msg);
      },
    );
    this.popups.set(windowId, popup);
    this.send(encodeSubscribeWindow(windowId));
  }

  /** Close a browser popup for a specific window */
  closeWindowPopup(windowId: number): void {
    const popup = this.popups.get(windowId);
    if (popup) {
      popup.close();
      this.popups.delete(windowId);
      this.send(encodeUnsubscribeWindow(windowId));
    }
  }

  /** Launch an application on the remote desktop */
  launchApp(command: string): void {
    this.send(encodeLaunchApp(command));
  }

  /**
   * Route an incoming video frame to the correct window popup's decoder.
   * Called when a frame with bit 1 set is received.
   */
  routeVideoFrame(windowId: number, data: Uint8Array, isKeyframe: boolean, pts: number): void {
    const popup = this.popups.get(windowId);
    if (popup) {
      popup.decodeFrame(data, isKeyframe, pts);
    } else {
      const n = (this._droppedFrameCounts.get(windowId) ?? 0) + 1;
      this._droppedFrameCounts.set(windowId, n);
      if (n === 1 || n % 120 === 0) {
        console.warn(
          `[coherence] no active stream for window ${windowId}; dropping coherence frame (#${n}). ` +
            `Open or Pop Out that window again if the list changed.`,
        );
      }
    }
  }
  private _droppedFrameCounts = new Map<number, number>();

  /**
   * Handle a window event message from the server (0x40 prefix).
   * Parses the binary data and updates internal state.
   */
  handleWindowEventData(data: Uint8Array): void {
    if (data.length < 2 || data[0] !== 0x40) return;

    const subtype = data[1];
    const view = new DataView(data.buffer, data.byteOffset, data.byteLength);

    switch (subtype) {
      case 0x01: // Snapshot
        this.handleSnapshot(data, view);
        break;
      case 0x02: // Added
        this.handleWindowAdded(data);
        break;
      case 0x03: // Removed
        if (data.length >= 6) {
          const wid = view.getUint32(2, false);
          this.handleWindowRemoved(wid);
        }
        break;
      case 0x04: // Geometry changed
        if (data.length >= 14) {
          const wid = view.getUint32(2, false);
          const x = view.getInt16(6, false);
          const y = view.getInt16(8, false);
          const w = view.getUint16(10, false);
          const h = view.getUint16(12, false);
          this.handleGeometryChanged(wid, x, y, w, h);
        }
        break;
      case 0x05: // Title changed
        if (data.length >= 8) {
          const wid = view.getUint32(2, false);
          const titleLen = view.getUint16(6, false);
          if (data.length >= 8 + titleLen) {
            const title = new TextDecoder().decode(data.slice(8, 8 + titleLen));
            this.handleTitleChanged(wid, title);
          }
        }
        break;
      case 0x06: // Visibility changed
        if (data.length >= 7) {
          const wid = view.getUint32(2, false);
          const visible = data[6] !== 0;
          this.handleVisibilityChanged(wid, visible);
        }
        break;
    }
  }

  /** Clean up all popups */
  destroy(): void {
    this.closeAllPopups();
    this.windows.clear();
    this.active = false;
  }

  // ── Private methods ────────────────────────────────────────────────

  private closeAllPopups(): void {
    for (const [wid, popup] of this.popups) {
      popup.close();
      this.send(encodeUnsubscribeWindow(wid));
    }
    this.popups.clear();
  }

  private handleSnapshot(data: Uint8Array, view: DataView): void {
    if (data.length < 4) return;
    const count = view.getUint16(2, false);
    let offset = 4;

    this.windows.clear();
    for (let i = 0; i < count; i++) {
      const result = deserializeWindowInfo(data, offset);
      if (!result) break;
      const [info, consumed] = result;
      this.windows.set(info.windowId, info);
      offset += consumed;
    }

    this.notifyWindowListChanged();
  }

  private handleWindowAdded(data: Uint8Array): void {
    const result = deserializeWindowInfo(data, 2);
    if (!result) return;
    const [info] = result;
    this.windows.set(info.windowId, info);
    this.notifyWindowListChanged();
  }

  private handleWindowRemoved(windowId: number): void {
    this.windows.delete(windowId);
    this.closeWindowPopup(windowId);
    this.notifyWindowListChanged();
  }

  private handleGeometryChanged(
    windowId: number,
    x: number,
    y: number,
    width: number,
    height: number,
  ): void {
    const info = this.windows.get(windowId);
    if (!info) return;
    if (x !== 0 || y !== 0) {
      info.x = x;
      info.y = y;
    }
    if (width !== 0 || height !== 0) {
      info.width = width;
      info.height = height;
      // Update popup size if open
      const popup = this.popups.get(windowId);
      if (popup) {
        popup.updateSize(width, height);
      }
    }
  }

  private handleTitleChanged(windowId: number, title: string): void {
    const info = this.windows.get(windowId);
    if (!info) return;
    info.title = title;
    const popup = this.popups.get(windowId);
    if (popup) {
      popup.updateTitle(title);
    }
    this.notifyWindowListChanged();
  }

  private handleVisibilityChanged(windowId: number, visible: boolean): void {
    const info = this.windows.get(windowId);
    if (!info) return;
    info.visible = visible;

    if (!visible) {
      // Window hidden — close popup and unsubscribe
      this.closeWindowPopup(windowId);
    }
    this.notifyWindowListChanged();
  }

  private notifyWindowListChanged(): void {
    this.handlers.onWindowListChanged?.(this.getWindows());
  }
}

/** Deserialize a WindowInfo from binary data at the given offset. */
function deserializeWindowInfo(
  data: Uint8Array,
  offset: number,
): [WindowInfo, number] | null {
  if (data.length < offset + 13) return null;

  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const windowId = view.getUint32(offset, false);
  const x = view.getInt16(offset + 4, false);
  const y = view.getInt16(offset + 6, false);
  const width = view.getUint16(offset + 8, false);
  const height = view.getUint16(offset + 10, false);
  const visible = data[offset + 12] !== 0;

  let pos = offset + 13;
  if (data.length < pos + 2) return null;
  const titleLen = view.getUint16(pos, false);
  pos += 2;
  if (data.length < pos + titleLen) return null;
  const title = new TextDecoder().decode(data.slice(pos, pos + titleLen));
  pos += titleLen;

  if (data.length < pos + 2) return null;
  const classLen = view.getUint16(pos, false);
  pos += 2;
  if (data.length < pos + classLen) return null;
  const appClass = new TextDecoder().decode(data.slice(pos, pos + classLen));
  pos += classLen;

  return [
    { windowId, title, x, y, width, height, visible, appClass },
    pos - offset,
  ];
}
