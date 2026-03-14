/**
 * WebRTC transport implementation.
 *
 * Uses WebRTC media tracks for video (rendered via a <video> element)
 * and a data channel for input (using the same binary protocol as WebTransport).
 */

import type { Transport } from './transport';

export interface WebRtcTransportOptions {
  /** Base HTTP URL for signaling (e.g. "http://localhost:4444") */
  signalingUrl: string;
  /** ICE server configuration */
  iceServers?: RTCIceServer[];
}

/**
 * WebRTC adapter implementing the Transport interface.
 *
 * Video arrives via a WebRTC media track and is exposed as a MediaStream.
 * Input is sent via a reliable, ordered data channel.
 */
export class WebRtcTransport implements Transport {
  private pc: RTCPeerConnection;
  private inputChannel: RTCDataChannel;
  private mediaStream: MediaStream;
  private dataCallback: ((data: Uint8Array) => void) | null = null;
  private closedResolve!: () => void;
  readonly closed: Promise<void>;
  private signalingUrl: string;
  private candidatePollInterval: number | undefined;

  private constructor(
    pc: RTCPeerConnection,
    inputChannel: RTCDataChannel,
    mediaStream: MediaStream,
    signalingUrl: string,
  ) {
    this.pc = pc;
    this.inputChannel = inputChannel;
    this.mediaStream = mediaStream;
    this.signalingUrl = signalingUrl;
    this.closed = new Promise<void>((resolve) => {
      this.closedResolve = resolve;
    });

    pc.onconnectionstatechange = () => {
      const state = pc.connectionState;
      if (state === 'closed' || state === 'failed' || state === 'disconnected') {
        this.closedResolve();
      }
    };
  }

  static async connect(options: WebRtcTransportOptions): Promise<WebRtcTransport> {
    const iceServers = options.iceServers ?? [
      { urls: 'stun:stun.l.google.com:19302' },
    ];

    const pc = new RTCPeerConnection({ iceServers });
    const mediaStream = new MediaStream();

    // Listen for incoming video tracks
    pc.ontrack = (event) => {
      for (const stream of event.streams) {
        for (const track of stream.getTracks()) {
          mediaStream.addTrack(track);
        }
      }
      if (event.track) {
        mediaStream.addTrack(event.track);
      }
    };

    // Create input data channel (ordered, reliable)
    const inputChannel = pc.createDataChannel('input', {
      ordered: true,
    });
    inputChannel.binaryType = 'arraybuffer';

    const transport = new WebRtcTransport(
      pc,
      inputChannel,
      mediaStream,
      options.signalingUrl,
    );

    // Collect local ICE candidates and send to server
    pc.onicecandidate = (event) => {
      if (event.candidate) {
        void fetch(`${options.signalingUrl}/webrtc/candidate`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            candidate: event.candidate.candidate,
            sdpMLineIndex: event.candidate.sdpMLineIndex,
          }),
        }).catch(() => {
          // Ignore network errors during ICE candidate exchange.
        });
      }
    };

    // Create and send SDP offer
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);

    const response = await fetch(`${options.signalingUrl}/webrtc/offer`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sdp: offer.sdp }),
    });

    if (!response.ok) {
      throw new Error(`Signaling failed: ${response.status} ${response.statusText}`);
    }

    const answer = (await response.json()) as { sdp: string };
    await pc.setRemoteDescription(
      new RTCSessionDescription({ type: 'answer', sdp: answer.sdp }),
    );

    // Poll for server-side ICE candidates
    transport.startCandidatePolling();

    // Wait for the input data channel to open
    await new Promise<void>((resolve, reject) => {
      if (inputChannel.readyState === 'open') {
        resolve();
        return;
      }
      inputChannel.onopen = () => resolve();
      inputChannel.onerror = (e) => reject(new Error(`Data channel error: ${e}`));
      // Timeout after 15 seconds
      setTimeout(() => reject(new Error('Data channel open timeout')), 15000);
    });

    return transport;
  }

  sendInput(data: Uint8Array): void {
    if (this.inputChannel.readyState === 'open') {
      // Copy into a new ArrayBuffer to satisfy RTCDataChannel.send() typing
      const buf = new ArrayBuffer(data.byteLength);
      new Uint8Array(buf).set(data);
      this.inputChannel.send(buf);
    }
  }

  onVideoFrame(_callback: (data: Uint8Array) => void): void {
    // Not used for WebRTC — video comes through media tracks
    // rendered via the <video> element.
  }

  getMediaStream(): MediaStream | null {
    return this.mediaStream;
  }

  onData(callback: (data: Uint8Array) => void): void {
    this.dataCallback = callback;
    // Listen for messages on additional data channels (e.g. clipboard)
    this.pc.ondatachannel = (event) => {
      const channel = event.channel;
      channel.binaryType = 'arraybuffer';
      channel.onmessage = (msg) => {
        if (msg.data instanceof ArrayBuffer) {
          this.dataCallback?.(new Uint8Array(msg.data));
        }
      };
    };
  }

  close(): void {
    this.stopCandidatePolling();
    try {
      this.inputChannel.close();
    } catch {
      // Ignore.
    }
    try {
      this.pc.close();
    } catch {
      // Ignore.
    }
    this.closedResolve();
  }

  private startCandidatePolling(): void {
    let consecutiveEmpty = 0;
    const poll = async () => {
      try {
        const response = await fetch(`${this.signalingUrl}/webrtc/candidates`);
        if (response.ok) {
          const candidates = (await response.json()) as Array<{
            candidate: string;
            sdpMLineIndex: number;
          }>;
          if (candidates.length > 0) {
            consecutiveEmpty = 0;
            for (const c of candidates) {
              await this.pc.addIceCandidate(
                new RTCIceCandidate({
                  candidate: c.candidate,
                  sdpMLineIndex: c.sdpMLineIndex,
                }),
              );
            }
          } else {
            consecutiveEmpty++;
          }
        }
      } catch {
        // Ignore network errors.
      }

      // Stop polling after 10 consecutive empty responses (ICE gathering likely done)
      if (consecutiveEmpty >= 10) {
        this.stopCandidatePolling();
      }
    };

    // Poll every 500ms
    this.candidatePollInterval = window.setInterval(() => void poll(), 500);
    // Also poll immediately
    void poll();
  }

  private stopCandidatePolling(): void {
    if (this.candidatePollInterval !== undefined) {
      clearInterval(this.candidatePollInterval);
      this.candidatePollInterval = undefined;
    }
  }
}
