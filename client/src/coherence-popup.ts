/**
 * Manages rendering a single X11 window's video stream.
 *
 * Supports two modes:
 * - **Inline mode** (default): renders within a container element in the main page.
 *   This avoids popup blockers and works reliably in all browsers.
 * - **Popup mode** (fallback): opens a separate browser window.
 *
 * Each instance contains a canvas that renders decoded H.264 frames from the server,
 * and captures input events (mouse, keyboard) to forward back to the server
 * with the correct coordinate mapping.
 */

import {
  attachInputListeners,
  encodeFocusWindow,
  encodeResizeWindow,
  encodeUnsubscribeWindow,
  type InputSender,
} from './input';
import type { WindowInfo } from './coherence';

export class WindowPopup {
  private popup: Window | null = null;
  private container: HTMLElement | null = null;
  private canvas: HTMLCanvasElement | null = null;
  private ctx: CanvasRenderingContext2D | null = null;
  private decoder: VideoDecoder | null = null;
  private cleanupInput: (() => void) | null = null;
  private cleanupPopupListener: (() => void) | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private windowId: number;
  private windowInfo: WindowInfo;
  private send: InputSender;
  private resizeTimeout: number | null = null;
  private frameCount = 0;
  private mode: 'inline' | 'popup' = 'inline';
  private onClose?: () => void;
  private onRequestKeyframe?: () => void;
  private waitingForKeyframe = true;
  private decoderAcceleration: VideoDecoderConfig['hardwareAcceleration'];
  private lastKeyframeRequestTime = 0;
  private unloadHandled = false;


  constructor(
    info: WindowInfo,
    send: InputSender,
    decoderAcceleration: VideoDecoderConfig['hardwareAcceleration'] = 'prefer-software',
    inlineParent?: HTMLElement,
    onClose?: () => void,
    onRequestKeyframe?: () => void,
  ) {
    this.windowId = info.windowId;
    this.windowInfo = info;
    this.send = send;
    this.onClose = onClose;
    this.onRequestKeyframe = onRequestKeyframe;
    this.decoderAcceleration = decoderAcceleration;

    if (inlineParent) {
      this.setupInline(info, inlineParent);
    } else {
      this.setupPopup(info);
    }

    if (!this.canvas) {
      console.error(`[sdk] wid=${info.windowId} NO CANVAS after setup, mode=${this.mode}`);
      return;
    }

    this.canvas.width = info.width;
    this.canvas.height = info.height;
    this.ctx = this.canvas.getContext('2d');

    if (!this.ctx) {
      console.error(`[coherence] wid=${info.windowId} failed to get 2d context, mode=${this.mode}`);
    }

    if (this.mode === 'inline') {
      this.initDecoder(info.windowId);
    } else {
      this.setupPopupMessageListener();
      this.sendPopupDecoderInit();
    }

    // Attach input listeners — coordinates are sent as absolute desktop coords
    // by adding the window's X/Y position offset
    const windowInfoRef = this.windowInfo;
    this.cleanupInput = attachInputListeners(this.canvas, send, () => {
      const rect = this.canvas!.getBoundingClientRect();
      const canvasWidth = rect.width || 1;
      const canvasHeight = rect.height || 1;
      return {
        scaleX: windowInfoRef.width / canvasWidth,
        scaleY: windowInfoRef.height / canvasHeight,
        offsetX: rect.left - (windowInfoRef.x / (windowInfoRef.width / canvasWidth)),
        offsetY: rect.top - (windowInfoRef.y / (windowInfoRef.height / canvasHeight)),
      };
    });

    // Focus canvas for keyboard input
    const canvasRef = this.canvas;
    setTimeout(() => canvasRef.focus(), 100);

    console.log(`[coherence] wid=${info.windowId} created: mode=${this.mode}, size=${info.width}x${info.height}`);
  }

  private setupInline(info: WindowInfo, parent: HTMLElement): void {
    this.mode = 'inline';

    // Create a container for this window's stream
    this.container = document.createElement('div');
    this.container.className = 'phantom-screen-inline-window';
    this.container.dataset.windowId = String(info.windowId);

    // Title bar
    const titleBar = document.createElement('div');
    titleBar.className = 'phantom-screen-inline-titlebar';
    titleBar.innerHTML = `
      <span class="phantom-screen-inline-title">${this.escapeHtml(info.title || info.appClass || 'Window')} [${info.windowId}]</span>
      <button class="phantom-screen-inline-close" title="Close stream">&times;</button>
    `;
    this.container.appendChild(titleBar);

    const closeBtn = titleBar.querySelector('.phantom-screen-inline-close');
    closeBtn?.addEventListener('click', () => {
      this.send(encodeUnsubscribeWindow(this.windowId));
      this.onClose?.();
    });

    // Canvas
    this.canvas = document.createElement('canvas');
    this.canvas.className = 'phantom-screen-inline-canvas';
    this.canvas.tabIndex = 0;
    this.container.appendChild(this.canvas);

    parent.appendChild(this.container);

    // Focus canvas on click anywhere in the container
    this.container.addEventListener('click', () => {
      this.canvas?.focus();
    });

    // Focus/raise the X11 window on mousedown (not just browser focus).
    // This ensures clicking a background window raises it immediately,
    // even if the canvas already has browser focus.
    this.canvas.addEventListener('mousedown', () => {
      this.send(encodeFocusWindow(this.windowId));
    });

    // Also send focus when the canvas gets browser focus (e.g., via Tab key)
    this.canvas.addEventListener('focus', () => {
      this.send(encodeFocusWindow(this.windowId));
    });
  }

  private setupPopup(info: WindowInfo): void {
    this.mode = 'popup';

    const features = `width=${info.width},height=${info.height},menubar=no,toolbar=no,location=no,status=no,resizable=yes`;
    const uniqueName = `phantom-window-${info.windowId}-${Date.now()}`;
    this.popup = window.open('', uniqueName, features);

    if (!this.popup) {
      console.warn(`[coherence] Popup blocked for window ${info.windowId}, cannot open in popup mode`);
      return;
    }

    // Build popup DOM programmatically instead of document.write() to avoid
    // Chrome's async document replacement which detaches the canvas and
    // fires spurious beforeunload events.
    const doc = this.popup.document;
    const windowLabel = `${info.title || info.appClass || 'Window'} [${info.windowId}]`;
    doc.title = windowLabel;

    const style = doc.createElement('style');
    style.textContent = `* { margin: 0; padding: 0; box-sizing: border-box; }
      html, body { width: 100%; height: 100%; overflow: hidden; background: #000; }
      .phantom-screen-popup-wid { position: absolute; top: 4px; left: 4px; z-index: 1; font: 11px monospace; color: rgba(255,255,255,0.8); pointer-events: none; }
      canvas { width: 100%; height: 100%; display: block; cursor: default; }`;
    doc.head.appendChild(style);

    const widLabel = doc.createElement('div');
    widLabel.className = 'phantom-screen-popup-wid';
    widLabel.textContent = `Window ${info.windowId}`;
    doc.body.appendChild(widLabel);

    const canvas = doc.createElement('canvas');
    canvas.id = 'stream-canvas';
    canvas.tabIndex = 0;
    doc.body.appendChild(canvas);
    this.canvas = canvas;

    // Watch for popup resize and tell the server to resize the X11 window
    // so the per-window pipeline captures at the new dimensions.
    this.resizeObserver = new ResizeObserver(() => {
      if (this.resizeTimeout !== null) {
        clearTimeout(this.resizeTimeout);
      }
      this.resizeTimeout = window.setTimeout(() => {
        if (this.popup && !this.popup.closed) {
          const w = this.popup.innerWidth;
          const h = this.popup.innerHeight;
          if (w > 0 && h > 0) {
            const evenW = w % 2 === 0 ? w : w + 1;
            const evenH = h % 2 === 0 ? h : h + 1;
            console.log(
              `[coherence] wid=${this.windowId} popup resize: raw=${w}x${h}, sending=${evenW}x${evenH} to server`,
            );
            this.send(encodeResizeWindow(this.windowId, evenW, evenH));
            // Mark that we're waiting for the pipeline restart's keyframe
            this.waitingForKeyframe = true;
          }
        }
      }, 250);
    });
    this.resizeObserver.observe(this.canvas);

    // Handle popup close — guard against spurious beforeunload fires
    this.popup.addEventListener('beforeunload', () => {
      if (this.unloadHandled) return;
      this.unloadHandled = true;
      console.log(`[coherence] Popup beforeunload for window ${this.windowId}`);
      this.send(encodeUnsubscribeWindow(this.windowId));
      this.onClose?.();
    });

    // Focus/raise the X11 window when the popup or canvas is interacted with
    this.popup.addEventListener('focus', () => {
      this.send(encodeFocusWindow(this.windowId));
    });

    // On mousedown: raise/focus the X11 window AND focus the canvas for keyboard
    canvas.addEventListener('mousedown', () => {
      this.send(encodeFocusWindow(this.windowId));
    });

    // Re-focus canvas on any click within the popup to ensure keyboard events work
    this.popup.document.addEventListener('click', () => {
      canvas.focus();
    });

    // Run decoder inside the popup so drawImage runs in the same document context.
    const script = doc.createElement('script');
    script.textContent = getPopupDecoderScript();
    doc.body.appendChild(script);
  }

  private sendPopupDecoderInit(): void {
    if (!this.popup || this.popup.closed) return;
    const send = (): void => {
      if (!this.popup || this.popup.closed) return;
      this.popup.postMessage(
        {
          type: 'phantom-coherence-init',
          decoderAcceleration: this.decoderAcceleration,
          width: this.windowInfo.width,
          height: this.windowInfo.height,
        },
        '*',
      );
      setTimeout(() => {
        if (this.popup && !this.popup.closed) this.popup.focus();
      }, 150);
    };
    setTimeout(send, 0);
  }

  private setupPopupMessageListener(): void {
    if (!this.popup) return;
    const handler = (e: MessageEvent): void => {
      if (e.source !== this.popup) return;
      const t = e.data?.type;
      if (t === 'phantom-coherence-requestKeyframe') {
        console.warn(`[coherence] wid=${this.windowId} popup decoder error, requesting keyframe`);
        this.requestKeyframe();
        return;
      }
    };
    window.addEventListener('message', handler);
    this.cleanupPopupListener = () => window.removeEventListener('message', handler);
  }

  /** Initialize or re-initialize the VideoDecoder */
  private initDecoder(windowId: number): void {
    try {
      this.decoder?.close();
    } catch {
      // may already be closed
    }

    this.waitingForKeyframe = true;
    this.frameCount = 0;

    this.decoder = new VideoDecoder({
      output: (frame: VideoFrame) => {
        if (this.mode === 'popup' && (!this.popup || this.popup.closed)) {
          frame.close();
          return;
        }
        if (this.canvas && this.ctx) {
          if (this.canvas.width !== frame.displayWidth || this.canvas.height !== frame.displayHeight) {
            console.log(`[coherence] wid=${windowId} canvas resize: ${frame.displayWidth}x${frame.displayHeight}`);
            this.canvas.width = frame.displayWidth;
            this.canvas.height = frame.displayHeight;
          }
          this.ctx.drawImage(frame, 0, 0);
        } else {
          console.error(`[coherence] wid=${windowId} decoder output but no canvas/ctx`);
        }
        frame.close();
      },
      error: (e: DOMException) => {
        console.error(`[coherence] wid=${windowId} decoder error: ${e.message}`);
        this.resetDecoder();
      },
    });

    this.decoder.configure({
      codec: 'avc1.42001f',
      hardwareAcceleration: this.decoderAcceleration,
      optimizeForLatency: true,
    });

    console.log(`[coherence] wid=${windowId} decoder configured (${this.decoderAcceleration})`);
  }

  /** Reset decoder after error and request a new keyframe */
  private resetDecoder(): void {
    console.warn(`[coherence] wid=${this.windowId} resetting decoder`);
    this.initDecoder(this.windowId);
    this.requestKeyframe();
  }

  /** Request a keyframe from server, debounced to max once per 2s */
  private requestKeyframe(): void {
    const now = Date.now();
    if (now - this.lastKeyframeRequestTime < 2000) return;
    this.lastKeyframeRequestTime = now;
    console.log(`[coherence] Requesting keyframe for window ${this.windowId}`);
    this.onRequestKeyframe?.();
  }

  /** Decode and render a video frame */
  decodeFrame(data: Uint8Array, isKeyframe: boolean, pts: number): void {
    if (this.mode === 'popup') {
      if (!this.popup || this.popup.closed) return;
      const buf = data.slice(0);
      this.popup.postMessage(
        { type: 'phantom-coherence-frame', data: buf.buffer, isKeyframe, pts },
        '*',
        [buf.buffer],
      );
      return;
    }

    if (!this.decoder) {
      if (this.frameCount === 0) {
        console.warn(`[coherence] wid=${this.windowId} frame received but no decoder`);
      }
      return;
    }

    // Auto-recover from closed decoder
    if (this.decoder.state === 'closed') {
      console.warn(`[coherence] wid=${this.windowId} decoder closed, resetting`);
      this.resetDecoder();
      return;
    }

    // Keyframe gating: drop delta frames until we receive a keyframe
    if (this.waitingForKeyframe) {
      if (!isKeyframe) {
        if (this.frameCount % 60 === 0) {
          console.debug(`[coherence] wid=${this.windowId} waiting for keyframe, dropped ${this.frameCount} delta frames`);
        }
        this.frameCount++;
        return;
      }
      this.waitingForKeyframe = false;
      console.log(`[coherence] wid=${this.windowId} received keyframe (${data.length}B) after ${this.frameCount} dropped deltas, starting decode`);
    }

    this.frameCount++;

    try {
      const chunk = new EncodedVideoChunk({
        type: isKeyframe ? 'key' : 'delta',
        timestamp: pts / 1000,
        data,
      });
      this.decoder.decode(chunk);
    } catch (e) {
      console.warn(`[coherence] wid=${this.windowId} decode exception:`, e);
      this.resetDecoder();
    }
  }

  /** Update the window title (label only; window ID stays) */
  updateTitle(title: string): void {
    if (this.mode === 'popup' && this.popup && !this.popup.closed) {
      this.popup.document.title = `${title} [${this.windowId}]`;
    }
    if (this.mode === 'inline' && this.container) {
      const titleEl = this.container.querySelector('.phantom-screen-inline-title');
      if (titleEl) titleEl.textContent = `${title} [${this.windowId}]`;
    }
  }

  /** Update canvas size when the X11 window is resized */
  updateSize(width: number, height: number): void {
    this.windowInfo.width = width;
    this.windowInfo.height = height;
    if (this.canvas) {
      this.canvas.width = width;
      this.canvas.height = height;
    }
  }

  /** Close and clean up resources */
  close(): void {
    // Neutralize callbacks first so that closing the popup window
    // doesn't trigger onClose/onRequestKeyframe asynchronously
    this.onClose = undefined;
    this.onRequestKeyframe = undefined;
    this.unloadHandled = true;

    if (this.resizeTimeout !== null) {
      clearTimeout(this.resizeTimeout);
    }
    this.resizeObserver?.disconnect();
    this.resizeObserver = null;
    this.cleanupInput?.();
    this.cleanupInput = null;
    this.cleanupPopupListener?.();
    this.cleanupPopupListener = null;

    try {
      this.decoder?.close();
    } catch {
      // Decoder may already be closed
    }
    this.decoder = null;

    if (this.popup && !this.popup.closed) {
      this.popup.close();
    }
    this.popup = null;

    if (this.container) {
      this.container.remove();
    }
    this.container = null;
    this.canvas = null;
    this.ctx = null;
  }

  private escapeHtml(text: string): string {
    return text
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }
}

/** Inline script for popup: decode and draw in the popup's own document so paint is visible. */
function getPopupDecoderScript(): string {
  return `
(function() {
  var decoder = null;
  var waitingForKeyframe = true;
  var canvas = null;
  var ctx = null;
  var codec = 'avc1.42001f';
  var firstPts = null;
  function notify(type, data) {
    if (window.opener) window.opener.postMessage(Object.assign({ type: type }, data || {}), '*');
  }
  window.addEventListener('message', function(e) {
    var d = e.data;
    if (!d || !d.type) return;
    if (d.type === 'phantom-coherence-init') {
      canvas = document.getElementById('stream-canvas');
      if (!canvas || !(ctx = canvas.getContext('2d'))) {
        console.error('[popup] init failed: no canvas/ctx');
        return;
      }
      if (d.width > 0 && d.height > 0) {
        canvas.width = d.width;
        canvas.height = d.height;
      }
      try { if (decoder) decoder.close(); } catch (_) {}
      waitingForKeyframe = true;
      firstPts = null;
      decoder = new VideoDecoder({
        output: function(frame) {
          if (!canvas || !ctx) { frame.close(); return; }
          if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
            canvas.width = frame.displayWidth;
            canvas.height = frame.displayHeight;
          }
          ctx.drawImage(frame, 0, 0);
          frame.close();
        },
        error: function(err) {
          var msg = err && err.message ? String(err.message) : String(err);
          console.error('[popup] decoder error:', msg);
          decoder = null;
          notify('phantom-coherence-requestKeyframe', { error: msg });
        }
      });
      try {
        decoder.configure({ codec: codec, hardwareAcceleration: d.decoderAcceleration || 'prefer-software', optimizeForLatency: true });
      } catch (err) {
        console.error('[popup] decoder configure failed:', err.message || err);
        decoder = null;
      }
      return;
    }
    if (d.type === 'phantom-coherence-frame') {
      if (!decoder || decoder.state === 'closed') {
        console.debug('[popup] frame received but decoder not ready, state=' + (decoder ? decoder.state : 'null'));
        return;
      }
      if (waitingForKeyframe && !d.isKeyframe) return;
      if (d.isKeyframe) {
        waitingForKeyframe = false;
        if (firstPts === null) firstPts = d.pts;
        console.log('[popup] keyframe received: ' + new Uint8Array(d.data).length + 'B');
      }
      if (!d.data) return;
      if (decoder.decodeQueueSize > 8) return;
      var raw = new Uint8Array(d.data);
      var tsUs = firstPts !== null ? (d.pts - firstPts) / 1000 : 0;
      try {
        decoder.decode(new EncodedVideoChunk({ type: d.isKeyframe ? 'key' : 'delta', timestamp: tsUs, data: raw.slice(0) }));
      } catch (err) {
        console.error('[popup] decode error:', err.message || err);
        decoder = null;
        notify('phantom-coherence-requestKeyframe', { error: err.message || String(err) });
      }
    }
  });
})();
`.trim();
}
