import { ClipboardSync } from './clipboard';
import { CoherenceController, type WindowInfo } from './coherence';
import { ControlManager } from './control';
import { createServerCertificateHashes, type ServerCertificateHash } from './hash';
import { attachInputListeners, type InputSender } from './input';
import { DEFAULT_SERVER_URL, renderTemplate } from './template';
import {
  getCanvasScale,
  getUIElements,
  setConnectionState,
  setupAutoHide,
  setupFullscreen,
  setupPointerLock,
  type ConnectionState,
  type UIElements,
} from './ui';

export type DecoderHardwareAcceleration = 'prefer-software' | 'prefer-hardware' | 'no-preference';

export interface PhantomScreenMountOptions {
  serverUrl?: string;
  serverCertificateHash?: string | Uint8Array;
  serverCertificateHashes?: ServerCertificateHash[];
  autoConnect?: boolean;
  title?: string;
  subtitle?: string;
  useShadowDom?: boolean;
  decoderHardwareAcceleration?: DecoderHardwareAcceleration;
  onStateChange?: (state: ConnectionState, message: string) => void;
}

type CleanupFn = () => void;

function toErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export class PhantomScreenClient {
  private readonly options: PhantomScreenMountOptions;
  private readonly renderRoot: ShadowRoot | HTMLElement;
  private readonly ui: UIElements;
  private readonly ctx: CanvasRenderingContext2D;
  private readonly cleanupUi: CleanupFn[];
  private state: ConnectionState = 'disconnected';
  private transport: WebTransport | null = null;
  private inputWriter: WritableStreamDefaultWriter<Uint8Array> | null = null;
  private decoder: VideoDecoder | null = null;
  private controlManager: ControlManager | null = null;
  private cleanupInput: CleanupFn | null = null;
  private clipboardSync = new ClipboardSync();
  private coherenceController: CoherenceController | null = null;

  constructor(root: HTMLElement, options: PhantomScreenMountOptions = {}) {
    this.options = options;
    this.renderRoot = this.getRenderRoot(root, options.useShadowDom !== false);
    this.ensureHostSizing(root);

    renderTemplate(this.renderRoot, {
      title: options.title,
      subtitle: options.subtitle,
      serverUrl: options.serverUrl ?? DEFAULT_SERVER_URL,
      certificateHash: typeof options.serverCertificateHash === 'string' ? options.serverCertificateHash : '',
    });

    this.ui = getUIElements(this.renderRoot);
    const context = this.ui.canvas.getContext('2d');
    if (!context) {
      throw new Error('Unable to create a 2D canvas context');
    }
    this.ctx = context;

    this.cleanupUi = [
      setupFullscreen(this.ui),
      setupPointerLock(this.ui),
      setupAutoHide(this.ui),
    ];

    // Coherence mode toggle
    const onCoherenceClick = () => {
      if (this.isCoherenceActive()) {
        this.disableCoherenceMode();
        this.ui.coherenceBtn.textContent = 'Coherence';
      } else {
        this.enableCoherenceMode();
        this.ui.coherenceBtn.textContent = 'Desktop';
      }
    };
    this.ui.coherenceBtn.addEventListener('click', onCoherenceClick);
    this.cleanupUi.push(() => {
      this.ui.coherenceBtn.removeEventListener('click', onCoherenceClick);
    });

    const onConnectClick = () => {
      void this.connect();
    };
    const onServerUrlKeydown = (event: KeyboardEvent) => {
      if (event.key === 'Enter') {
        void this.connect();
      }
    };
    const onCertHashKeydown = (event: KeyboardEvent) => {
      if (event.key === 'Enter') {
        void this.connect();
      }
    };

    this.ui.connectBtn.addEventListener('click', onConnectClick);
    this.ui.serverUrlInput.addEventListener('keydown', onServerUrlKeydown);
    this.ui.certHashInput.addEventListener('keydown', onCertHashKeydown);

    this.cleanupUi.push(() => {
      this.ui.connectBtn.removeEventListener('click', onConnectClick);
      this.ui.serverUrlInput.removeEventListener('keydown', onServerUrlKeydown);
      this.ui.certHashInput.removeEventListener('keydown', onCertHashKeydown);
    });

    this.updateState('disconnected', 'Disconnected');

    if (options.autoConnect) {
      void this.connect();
    }
  }

  getState(): ConnectionState {
    return this.state;
  }

  async connect(serverUrl = this.ui.serverUrlInput.value.trim()): Promise<void> {
    this.disconnect(false);

    if (!serverUrl) {
      this.updateState('error', 'Please enter a server URL');
      return;
    }

    this.updateState('connecting', 'Connecting...');
    this.ui.serverUrlInput.value = serverUrl;

    try {
      if (typeof WebTransport === 'undefined') {
        throw new Error('WebTransport is not available in this browser');
      }

      const transportOptions = this.resolveTransportOptions();
      this.transport = transportOptions
        ? new WebTransport(serverUrl, transportOptions)
        : new WebTransport(serverUrl);

      const currentTransport = this.transport;
      currentTransport.closed
        .then(() => {
          if (this.transport === currentTransport) {
            this.disconnect(false);
            this.updateState('disconnected', 'Connection closed');
          }
        })
        .catch((error) => {
          if (this.transport === currentTransport) {
            this.disconnect(false);
            this.updateState('error', `Connection lost: ${toErrorMessage(error)}`);
          }
        });

      await currentTransport.ready;
      this.updateState('connected', 'Connected');

      const biStream = await currentTransport.createBidirectionalStream();
      this.inputWriter = biStream.writable.getWriter();

      const send: InputSender = (data: Uint8Array) => {
        this.inputWriter?.write(data).catch(() => {
          // Ignore writes after disconnects.
        });
      };

      this.controlManager = new ControlManager(send, this.ui);
      this.coherenceController = new CoherenceController(send, {
        onWindowListChanged: (windows) => {
          this.updateCoherenceUI(windows);
        },
      }, this.options.decoderHardwareAcceleration ?? 'prefer-software');

      // Set inline parent for coherence window streams
      const inlineStreams = this.renderRoot.querySelector<HTMLElement>('[data-phantom-screen="inline-streams"]');
      if (inlineStreams) {
        this.coherenceController.setInlineParent(inlineStreams);
      }

      this.setupDecoder();

      this.cleanupInput = attachInputListeners(
        this.ui.canvas,
        send,
        () => getCanvasScale(
          this.ui.canvas,
          this.controlManager?.getRemoteWidth() ?? this.ui.canvas.width ?? 1920,
          this.controlManager?.getRemoteHeight() ?? this.ui.canvas.height ?? 1080,
        ),
      );

      this.ui.canvas.focus();

      void this.readVideoStreams(currentTransport);
      void this.readInputResponses(biStream.readable.getReader());
      void this.loadLaunchApps(serverUrl);
    } catch (error) {
      this.disconnect(false);
      this.updateState('error', `Failed to connect: ${toErrorMessage(error)}`);
    }
  }

  /** Enable coherence mode — each X11 window becomes a separate browser popup */
  enableCoherenceMode(): void {
    this.coherenceController?.enableCoherenceMode();
    // Hide the main canvas and show the coherence panel
    this.ui.canvas.style.display = 'none';
    const panel = this.renderRoot.querySelector<HTMLElement>('[data-phantom-screen="coherence-panel"]');
    if (panel) panel.style.display = 'block';
  }

  /** Disable coherence mode — return to full-desktop streaming */
  disableCoherenceMode(): void {
    this.coherenceController?.disableCoherenceMode();
    this.ui.canvas.style.display = '';
    const panel = this.renderRoot.querySelector<HTMLElement>('[data-phantom-screen="coherence-panel"]');
    if (panel) panel.style.display = 'none';
  }

  /** Open a popup for a specific window */
  openWindowPopup(windowId: number): void {
    this.coherenceController?.openWindowPopup(windowId);
  }

  /** Launch an app on the remote desktop */
  launchApp(command: string): void {
    this.coherenceController?.launchApp(command);
  }

  /** Check if coherence mode is active */
  isCoherenceActive(): boolean {
    return this.coherenceController?.isActive() ?? false;
  }

  disconnect(updateState = true): void {
    this.cleanupInput?.();
    this.cleanupInput = null;

    this.coherenceController?.destroy();
    this.coherenceController = null;

    this.controlManager?.destroy();
    this.controlManager = null;

    try {
      this.decoder?.close();
    } catch {
      // Decoder may already be closed.
    }
    this.decoder = null;

    const writer = this.inputWriter;
    this.inputWriter = null;
    void writer?.close().catch(() => {
      // Stream already closed.
    });

    const transport = this.transport;
    this.transport = null;
    try {
      transport?.close();
    } catch {
      // Ignore close races.
    }

    this.clipboardSync = new ClipboardSync();

    if (updateState) {
      this.updateState('disconnected', 'Disconnected');
    }
  }

  destroy(): void {
    this.disconnect(false);
    for (const cleanup of this.cleanupUi) {
      cleanup();
    }
    this.renderRoot.innerHTML = '';
  }

  private getRenderRoot(root: HTMLElement, useShadowDom: boolean): ShadowRoot | HTMLElement {
    if (!useShadowDom) {
      return root;
    }
    if (root.shadowRoot) {
      return root.shadowRoot;
    }
    return root.attachShadow({ mode: 'open' });
  }

  private ensureHostSizing(root: HTMLElement): void {
    if (!root.style.display) {
      root.style.display = 'block';
    }
    if (!root.style.width) {
      root.style.width = '100%';
    }
    if (!root.style.height) {
      root.style.minHeight = root.style.minHeight || '360px';
    }
  }

  private updateState(state: ConnectionState, message: string): void {
    this.state = state;
    setConnectionState(this.ui, state, message);
    this.options.onStateChange?.(state, message);
  }

  private resolveTransportOptions():
    | ConstructorParameters<typeof WebTransport>[1]
    | undefined {
    const manualHash = this.ui.certHashInput.value.trim();
    if (manualHash) {
      return {
        serverCertificateHashes: createServerCertificateHashes(manualHash),
      };
    }

    if (this.options.serverCertificateHashes?.length) {
      return {
        serverCertificateHashes: this.options.serverCertificateHashes,
      };
    }

    const singleHash = createServerCertificateHashes(this.options.serverCertificateHash);
    if (singleHash) {
      return {
        serverCertificateHashes: singleHash,
      };
    }

    return undefined;
  }

  private setupDecoder(): void {
    if (typeof VideoDecoder === 'undefined') {
      throw new Error('WebCodecs VideoDecoder is not available in this browser');
    }

    this.decoder = new VideoDecoder({
      output: (frame: VideoFrame) => {
        const startDraw = performance.now();

        if (this.ui.canvas.width !== frame.displayWidth || this.ui.canvas.height !== frame.displayHeight) {
          this.ui.canvas.width = frame.displayWidth;
          this.ui.canvas.height = frame.displayHeight;
          this.controlManager?.setRemoteResolution(frame.displayWidth, frame.displayHeight);
        }

        this.ctx.drawImage(frame, 0, 0);
        frame.close();

        const drawTime = performance.now() - startDraw;
        this.controlManager?.recordFrame(drawTime);
      },
      error: () => {
        this.controlManager?.requestKeyframe();
      },
    });

    this.decoder.configure({
      codec: 'avc1.42001f',
      hardwareAcceleration: this.options.decoderHardwareAcceleration ?? 'prefer-software',
      optimizeForLatency: true,
    });
  }

  private async readVideoStreams(transport: WebTransport): Promise<void> {
    const reader = transport.incomingUnidirectionalStreams.getReader();

    try {
      while (true) {
        const { value: stream, done } = await reader.read();
        if (done) {
          break;
        }
        void this.processVideoStream(stream);
      }
    } catch {
      // Ignore stream shutdowns during disconnect.
    } finally {
      reader.releaseLock();
    }
  }

  private async processVideoStream(stream: ReadableStream<Uint8Array>): Promise<void> {
    const reader = stream.getReader();
    const chunks: Uint8Array[] = [];
    let totalLength = 0;

    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) {
          break;
        }
        chunks.push(value);
        totalLength += value.length;
      }
    } catch {
      return;
    } finally {
      reader.releaseLock();
    }

    // Assemble all chunks into a single buffer
    const data = new Uint8Array(totalLength);
    let offset = 0;
    for (const chunk of chunks) {
      data.set(chunk, offset);
      offset += chunk.length;
    }

    // Check for window events FIRST — they can be shorter than 13 bytes
    // (e.g., Removed = 6 bytes, VisibilityChanged = 7 bytes)
    if (data.length >= 2 && data[0] === 0x40) {
      this.coherenceController?.handleWindowEventData(data);
      return;
    }

    if (totalLength < 13) {
      return;
    }

    const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
    const flags = data[0];
    const isKeyframe = (flags & 0x01) !== 0;
    const isWindowFrame = (flags & 0x02) !== 0;

    if (isWindowFrame) {
      // Per-window coherence frame: [flags:u8][window_id:u32][pts:u64][len:u32][data]
      if (data.length < 17) {
        console.warn('[sdk] Window frame too short:', data.length);
        return;
      }
      const windowId = view.getUint32(1, false);
      const ptsHigh = view.getUint32(5, false);
      const ptsLow = view.getUint32(9, false);
      const pts = ptsHigh * 0x100000000 + ptsLow;
      const payloadLength = view.getUint32(13, false);

      if (data.length < 17 + payloadLength) {
        console.warn('[sdk] Window frame payload truncated:', data.length, 'expected', 17 + payloadLength);
        return;
      }

      this.coherenceController?.routeVideoFrame(
        windowId,
        data.slice(17, 17 + payloadLength),
        isKeyframe,
        pts,
      );
      return;
    }

    // Regular full-desktop frame: [flags:u8][pts:u64][len:u32][data]
    const ptsHigh = view.getUint32(1, false);
    const ptsLow = view.getUint32(5, false);
    const pts = ptsHigh * 0x100000000 + ptsLow;
    const payloadLength = view.getUint32(9, false);

    if (data.length < 13 + payloadLength || !this.decoder || this.decoder.state === 'closed') {
      return;
    }

    try {
      const chunk = new EncodedVideoChunk({
        type: isKeyframe ? 'key' : 'delta',
        timestamp: pts / 1000,
        data: data.slice(13, 13 + payloadLength),
      });
      this.decoder.decode(chunk);
    } catch {
      this.controlManager?.requestKeyframe();
    }
  }

  private async loadLaunchApps(serverUrl: string): Promise<void> {
    try {
      // Derive the HTTP URL from the WebTransport URL (port + 1)
      const url = new URL(serverUrl);
      const port = parseInt(url.port || '4443', 10) + 1;
      const httpUrl = `http://${url.hostname}:${port}/api/launch-apps`;
      const resp = await fetch(httpUrl);
      if (!resp.ok) return;
      const apps: string[] = await resp.json();

      const grid = this.renderRoot.querySelector<HTMLElement>('[data-phantom-screen="launch-grid"]');
      if (!grid) return;

      grid.innerHTML = '';
      for (const app of apps) {
        const btn = document.createElement('button');
        btn.className = 'phantom-screen-launch-btn';
        // Show friendly label (first word, capitalized) but send full command on click
        const label = app.trim().split(/\s+/)[0] || app;
        btn.textContent = label.charAt(0).toUpperCase() + label.slice(1).toLowerCase();
        btn.title = app;
        btn.addEventListener('click', () => {
          this.launchApp(app);
        });
        grid.appendChild(btn);
      }
    } catch {
      // Non-critical — launch apps just won't be populated
    }
  }

  private updateCoherenceUI(windows: WindowInfo[]): void {
    const list = this.renderRoot.querySelector<HTMLElement>('[data-phantom-screen="window-list"]');
    if (!list) return;

    list.innerHTML = '';
    for (const win of windows) {
      if (!win.visible) continue;
      const item = document.createElement('div');
      item.className = 'phantom-screen-window-item';
      const title = (win.title || win.appClass || 'Window').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
      item.innerHTML = `
        <span class="phantom-screen-window-title">${title} [${win.windowId}]</span>
        <span class="phantom-screen-window-size">${win.width}x${win.height}</span>
        <button class="phantom-screen-toolbar-btn phantom-screen-window-open-btn" data-window-id="${win.windowId}">Open</button>
        <button class="phantom-screen-toolbar-btn phantom-screen-window-popout-btn" data-window-id="${win.windowId}">Pop Out</button>
      `;
      const openBtn = item.querySelector('.phantom-screen-window-open-btn');
      openBtn?.addEventListener('click', () => {
        this.coherenceController?.openWindowPopup(win.windowId);
      });
      const popoutBtn = item.querySelector('.phantom-screen-window-popout-btn');
      popoutBtn?.addEventListener('click', () => {
        this.coherenceController?.openWindowAsPopup(win.windowId);
      });
      list.appendChild(item);
    }
  }

  private async readInputResponses(
    reader: ReadableStreamDefaultReader<Uint8Array>,
  ): Promise<void> {
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) {
          break;
        }

        if (value && value.length >= 5 && value[0] === 0x20) {
          const view = new DataView(value.buffer, value.byteOffset, value.byteLength);
          const textLength = view.getUint32(1, false);
          if (value.length >= 5 + textLength) {
            const text = new TextDecoder().decode(value.slice(5, 5 + textLength));
            await this.clipboardSync.receiveClipboard(text);
          }
        }
      }
    } catch {
      // Ignore stream shutdowns during disconnect.
    } finally {
      reader.releaseLock();
    }
  }
}

export function mountPhantomScreen(
  root: HTMLElement,
  options?: PhantomScreenMountOptions,
): PhantomScreenClient {
  return new PhantomScreenClient(root, options);
}
