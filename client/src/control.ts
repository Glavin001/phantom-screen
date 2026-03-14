/**
 * Control message handling: keyframe requests, stats tracking, resolution negotiation.
 */

import { encodeKeyframeRequest, encodeBitrateRequest, encodeResolutionRequest, type InputSender } from './input';
import { updateStats, updateResolution, type UIElements } from './ui';

export class ControlManager {
  private frameCount = 0;
  private fpsFrameCount = 0;
  private lastFpsTime = performance.now();
  private currentFps = 0;
  private totalDecodeTime = 0;
  private decodeCount = 0;
  private remoteWidth = 1920;
  private remoteHeight = 1080;
  private send: InputSender;
  private ui: UIElements;
  private statsInterval: number;
  private resizeTimeout: number | undefined;
  private resizeObserver: ResizeObserver | null = null;
  private readonly onKeyframeClick: () => void;
  private readonly onWindowResize: () => void;

  constructor(send: InputSender, ui: UIElements) {
    this.send = send;
    this.ui = ui;
    this.onKeyframeClick = () => this.requestKeyframe();
    this.onWindowResize = () => this.scheduleResizeRequest();

    // Update stats display every second
    this.statsInterval = window.setInterval(() => this.updateFps(), 1000);

    // Setup keyframe button
    ui.keyframeBtn.addEventListener('click', this.onKeyframeClick);

    // Setup resize detection
    this.setupResizeObserver();
  }

  /** Record a decoded frame for stats */
  recordFrame(decodeTimeMs: number) {
    this.frameCount++;
    this.fpsFrameCount++;
    this.totalDecodeTime += decodeTimeMs;
    this.decodeCount++;
  }

  /** Request a keyframe from the server */
  requestKeyframe() {
    this.send(encodeKeyframeRequest());
  }

  /** Request a bitrate change */
  setBitrate(kbps: number) {
    this.send(encodeBitrateRequest(kbps));
  }

  /** Set the remote desktop resolution (from initial stream info) */
  setRemoteResolution(width: number, height: number) {
    this.remoteWidth = width;
    this.remoteHeight = height;
    updateResolution(this.ui, width, height);

    // Size the canvas to match
    this.ui.canvas.width = width;
    this.ui.canvas.height = height;
  }

  getRemoteWidth(): number { return this.remoteWidth; }
  getRemoteHeight(): number { return this.remoteHeight; }

  /** Clean up */
  destroy() {
    clearInterval(this.statsInterval);
    clearTimeout(this.resizeTimeout);
    this.ui.keyframeBtn.removeEventListener('click', this.onKeyframeClick);
    window.removeEventListener('resize', this.onWindowResize);
    this.resizeObserver?.disconnect();
    this.resizeObserver = null;
  }

  private updateFps() {
    const now = performance.now();
    const elapsed = (now - this.lastFpsTime) / 1000;
    this.currentFps = Math.round(this.fpsFrameCount / elapsed);
    this.fpsFrameCount = 0;
    this.lastFpsTime = now;

    const avgDecode = this.decodeCount > 0 ? this.totalDecodeTime / this.decodeCount : 0;
    updateStats(this.ui, this.currentFps, this.frameCount, avgDecode);

    // Reset decode averages periodically
    this.totalDecodeTime = 0;
    this.decodeCount = 0;
  }

  private scheduleResizeRequest() {
    clearTimeout(this.resizeTimeout);
    this.resizeTimeout = window.setTimeout(() => {
      this.sendResizeIfChanged();
    }, 100); // 100ms debounce for fast but not excessive updates
  }

  private sendResizeIfChanged() {
    // Use the container's size rather than window.innerWidth so we track the
    // actual space available for the video, even in embedded/pop-out scenarios.
    const rect = this.ui.container.getBoundingClientRect();
    const width = Math.round(rect.width);
    const height = Math.round(rect.height);

    // Skip tiny or zero sizes (e.g. hidden element)
    if (width < 64 || height < 64) return;

    // Only request if meaningfully different (> 16px threshold avoids sub-pixel noise)
    if (
      Math.abs(width - this.remoteWidth) > 16 ||
      Math.abs(height - this.remoteHeight) > 16
    ) {
      this.send(encodeResolutionRequest(width, height));
    }
  }

  private setupResizeObserver() {
    // Use ResizeObserver for precise container size tracking (handles pop-out,
    // embedded iframes, and CSS layout changes that window.resize misses)
    if (typeof ResizeObserver !== 'undefined') {
      this.resizeObserver = new ResizeObserver(() => {
        this.scheduleResizeRequest();
      });
      this.resizeObserver.observe(this.ui.container);
    }

    // Also listen to window resize as a fallback
    window.addEventListener('resize', this.onWindowResize);
  }
}
