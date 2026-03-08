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
  private readonly onKeyframeClick: () => void;
  private readonly onWindowResize: () => void;

  constructor(send: InputSender, ui: UIElements) {
    this.send = send;
    this.ui = ui;
    this.onKeyframeClick = () => this.requestKeyframe();
    this.onWindowResize = () => {
      clearTimeout(this.resizeTimeout);
      this.resizeTimeout = window.setTimeout(() => {
        // Only request if significantly different from current
        const width = window.innerWidth;
        const height = window.innerHeight;
        if (
          Math.abs(width - this.remoteWidth) > 100 ||
          Math.abs(height - this.remoteHeight) > 100
        ) {
          this.send(encodeResolutionRequest(width, height));
        }
      }, 500);
    };

    // Update stats display every second
    this.statsInterval = window.setInterval(() => this.updateFps(), 1000);

    // Setup keyframe button
    ui.keyframeBtn.addEventListener('click', this.onKeyframeClick);

    // Setup resize observer
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

  private setupResizeObserver() {
    // When the browser window resizes, we could request a resolution change
    window.addEventListener('resize', this.onWindowResize);
  }
}
