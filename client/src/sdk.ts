import { ClipboardSync } from './clipboard';
import { ControlManager } from './control';
import { type ServerCertificateHash } from './hash';
import { attachInputListeners, type InputSender } from './input';
import { DEFAULT_SERVER_URL, renderTemplate } from './template';
import { type Transport, type TransportType, WebTransportAdapter } from './transport';
import { WebRtcTransport } from './webrtc-transport';
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
  /** Transport selection: 'auto' tries WebTransport first, falls back to WebRTC */
  transport?: TransportType;
  /** ICE servers for WebRTC (default: Google STUN) */
  iceServers?: RTCIceServer[];
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
  private activeTransport: Transport | null = null;
  private decoder: VideoDecoder | null = null;
  private controlManager: ControlManager | null = null;
  private cleanupInput: CleanupFn | null = null;
  private clipboardSync = new ClipboardSync();
  private videoElement: HTMLVideoElement | null = null;

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
      const transportMode = this.options.transport ?? 'auto';
      const transport = await this.createTransport(transportMode, serverUrl);
      this.activeTransport = transport;

      // Handle transport close
      transport.closed
        .then(() => {
          if (this.activeTransport === transport) {
            this.disconnect(false);
            this.updateState('disconnected', 'Connection closed');
          }
        })
        .catch((error) => {
          if (this.activeTransport === transport) {
            this.disconnect(false);
            this.updateState('error', `Connection lost: ${toErrorMessage(error)}`);
          }
        });

      this.updateState('connected', 'Connected');

      const send: InputSender = (data: Uint8Array) => {
        this.activeTransport?.sendInput(data);
      };

      this.controlManager = new ControlManager(send, this.ui);

      // Set up video rendering based on transport type
      const mediaStream = transport.getMediaStream();
      if (mediaStream) {
        // WebRTC: render video via <video> element
        this.setupWebRtcVideo(mediaStream);
      } else {
        // WebTransport: decode video frames via WebCodecs
        this.setupDecoder();
        transport.onVideoFrame((data) => this.handleVideoFrame(data));
      }

      // Handle incoming data (clipboard from server)
      transport.onData((data) => {
        void this.handleIncomingData(data);
      });

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
    } catch (error) {
      this.disconnect(false);
      this.updateState('error', `Failed to connect: ${toErrorMessage(error)}`);
    }
  }

  disconnect(updateState = true): void {
    this.cleanupInput?.();
    this.cleanupInput = null;

    this.controlManager?.destroy();
    this.controlManager = null;

    try {
      this.decoder?.close();
    } catch {
      // Decoder may already be closed.
    }
    this.decoder = null;

    if (this.videoElement) {
      this.videoElement.srcObject = null;
      this.videoElement.remove();
      this.videoElement = null;
    }

    const transport = this.activeTransport;
    this.activeTransport = null;
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

  private async createTransport(mode: TransportType, serverUrl: string): Promise<Transport> {
    const useWebTransport =
      mode === 'webtransport' ||
      (mode === 'auto' && typeof WebTransport !== 'undefined');

    if (useWebTransport) {
      return WebTransportAdapter.connect({
        serverUrl,
        manualHash: this.ui.certHashInput.value.trim() || undefined,
        certificateHashes: this.options.serverCertificateHashes,
        singleHash: this.options.serverCertificateHash,
      });
    }

    // WebRTC fallback
    // Derive the signaling URL from the server URL
    // Server URL is typically https://host:4443, HTTP is on port+1 (4444)
    const signalingUrl = this.deriveSignalingUrl(serverUrl);
    return WebRtcTransport.connect({
      signalingUrl,
      iceServers: this.options.iceServers,
    });
  }

  private deriveSignalingUrl(serverUrl: string): string {
    try {
      const url = new URL(serverUrl);
      const port = parseInt(url.port || '4443', 10);
      // HTTP server runs on port+1
      return `${url.protocol === 'https:' ? 'https' : 'http'}://${url.hostname}:${port + 1}`;
    } catch {
      // Fallback: assume same origin
      return window.location.origin;
    }
  }

  private setupWebRtcVideo(mediaStream: MediaStream): void {
    // Create a hidden <video> element to receive the WebRTC media stream
    const video = document.createElement('video');
    video.autoplay = true;
    video.playsInline = true;
    video.muted = true;
    video.srcObject = mediaStream;
    video.style.display = 'none';
    this.renderRoot.appendChild(video);
    this.videoElement = video;

    // Draw video frames to the canvas
    const drawFrame = () => {
      if (!this.videoElement || this.videoElement.paused || this.videoElement.ended) return;

      if (video.videoWidth > 0 && video.videoHeight > 0) {
        if (this.ui.canvas.width !== video.videoWidth || this.ui.canvas.height !== video.videoHeight) {
          this.ui.canvas.width = video.videoWidth;
          this.ui.canvas.height = video.videoHeight;
          this.controlManager?.setRemoteResolution(video.videoWidth, video.videoHeight);
        }

        const startDraw = performance.now();
        this.ctx.drawImage(video, 0, 0);
        const drawTime = performance.now() - startDraw;
        this.controlManager?.recordFrame(drawTime);
      }

      requestAnimationFrame(drawFrame);
    };

    video.addEventListener('playing', () => {
      requestAnimationFrame(drawFrame);
    });

    void video.play().catch(() => {
      // Autoplay may be blocked; user interaction needed.
    });
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

  private handleVideoFrame(data: Uint8Array): void {
    if (data.length < 13) return;

    const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
    const flags = data[0];
    const isKeyframe = (flags & 0x01) !== 0;
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

  private async handleIncomingData(value: Uint8Array): Promise<void> {
    if (value && value.length >= 5 && value[0] === 0x20) {
      const view = new DataView(value.buffer, value.byteOffset, value.byteLength);
      const textLength = view.getUint32(1, false);
      if (value.length >= 5 + textLength) {
        const text = new TextDecoder().decode(value.slice(5, 5 + textLength));
        await this.clipboardSync.receiveClipboard(text);
      }
    }
  }
}

export function mountPhantomScreen(
  root: HTMLElement,
  options?: PhantomScreenMountOptions,
): PhantomScreenClient {
  return new PhantomScreenClient(root, options);
}
