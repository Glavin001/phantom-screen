/**
 * Transport abstraction layer.
 *
 * Defines a common interface for WebTransport and WebRTC transports,
 * allowing the SDK to work with either one interchangeably.
 */

import { createServerCertificateHashes, type ServerCertificateHash } from './hash';

export type TransportType = 'auto' | 'webtransport' | 'webrtc';

/**
 * Common transport interface for sending input and receiving video.
 *
 * For WebTransport: video arrives as binary frames (same as before).
 * For WebRTC: video arrives as a MediaStream on a <video> element;
 *   `onVideoFrame` is unused and input goes through a data channel.
 */
export interface Transport {
  /** Send binary input data to the server */
  sendInput(data: Uint8Array): void;

  /**
   * Register callback for incoming binary video frame data.
   * Used by WebTransport. WebRTC uses media tracks instead, so this
   * callback may never fire for WebRTC.
   */
  onVideoFrame(callback: (data: Uint8Array) => void): void;

  /**
   * For WebRTC: returns the MediaStream for the video track.
   * Returns null for WebTransport.
   */
  getMediaStream(): MediaStream | null;

  /** Register callback for incoming binary data (e.g. clipboard from server) */
  onData(callback: (data: Uint8Array) => void): void;

  /** Close the transport */
  close(): void;

  /** Promise that resolves when the transport connection is closed */
  readonly closed: Promise<void>;
}

export interface WebTransportOptions {
  serverUrl: string;
  certificateHashes?: ServerCertificateHash[];
  manualHash?: string;
  singleHash?: string | Uint8Array;
}

/**
 * WebTransport adapter implementing the Transport interface.
 * Wraps the browser WebTransport API.
 */
export class WebTransportAdapter implements Transport {
  private transport: WebTransport;
  private inputWriter: WritableStreamDefaultWriter<Uint8Array> | null = null;
  private videoCallback: ((data: Uint8Array) => void) | null = null;
  private dataCallback: ((data: Uint8Array) => void) | null = null;
  readonly closed: Promise<void>;

  constructor(transport: WebTransport) {
    this.transport = transport;
    this.closed = transport.closed.then(() => {}).catch(() => {});
  }

  static async connect(options: WebTransportOptions): Promise<WebTransportAdapter> {
    if (typeof WebTransport === 'undefined') {
      throw new Error('WebTransport is not available in this browser');
    }

    const transportOptions = resolveTransportOptions(options);
    const transport = transportOptions
      ? new WebTransport(options.serverUrl, transportOptions)
      : new WebTransport(options.serverUrl);

    await transport.ready;

    const adapter = new WebTransportAdapter(transport);

    // Open a bidirectional stream for input
    const biStream = await transport.createBidirectionalStream();
    adapter.inputWriter = biStream.writable.getWriter();

    // Start reading video streams
    adapter.readVideoStreams();

    // Start reading input responses (clipboard data from server)
    adapter.readInputResponses(biStream.readable.getReader());

    return adapter;
  }

  sendInput(data: Uint8Array): void {
    this.inputWriter?.write(data).catch(() => {
      // Ignore writes after disconnects.
    });
  }

  onVideoFrame(callback: (data: Uint8Array) => void): void {
    this.videoCallback = callback;
  }

  getMediaStream(): MediaStream | null {
    return null;
  }

  onData(callback: (data: Uint8Array) => void): void {
    this.dataCallback = callback;
  }

  close(): void {
    const writer = this.inputWriter;
    this.inputWriter = null;
    void writer?.close().catch(() => {});

    try {
      this.transport.close();
    } catch {
      // Ignore close races.
    }
  }

  private readVideoStreams(): void {
    const reader = this.transport.incomingUnidirectionalStreams.getReader();

    const readLoop = async () => {
      try {
        while (true) {
          const { value: stream, done } = await reader.read();
          if (done) break;
          void this.processVideoStream(stream);
        }
      } catch {
        // Ignore stream shutdowns during disconnect.
      } finally {
        reader.releaseLock();
      }
    };

    void readLoop();
  }

  private async processVideoStream(stream: ReadableStream<Uint8Array>): Promise<void> {
    const reader = stream.getReader();
    const chunks: Uint8Array[] = [];
    let totalLength = 0;

    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        chunks.push(value);
        totalLength += value.length;
      }
    } catch {
      return;
    } finally {
      reader.releaseLock();
    }

    if (totalLength < 13) return;

    const data = new Uint8Array(totalLength);
    let offset = 0;
    for (const chunk of chunks) {
      data.set(chunk, offset);
      offset += chunk.length;
    }

    this.videoCallback?.(data);
  }

  private readInputResponses(reader: ReadableStreamDefaultReader<Uint8Array>): void {
    const readLoop = async () => {
      try {
        while (true) {
          const { value, done } = await reader.read();
          if (done) break;
          if (value) {
            this.dataCallback?.(value);
          }
        }
      } catch {
        // Ignore stream shutdowns during disconnect.
      } finally {
        reader.releaseLock();
      }
    };

    void readLoop();
  }
}

function resolveTransportOptions(
  options: WebTransportOptions,
): ConstructorParameters<typeof WebTransport>[1] | undefined {
  if (options.manualHash) {
    return {
      serverCertificateHashes: createServerCertificateHashes(options.manualHash),
    };
  }

  if (options.certificateHashes?.length) {
    return {
      serverCertificateHashes: options.certificateHashes,
    };
  }

  const singleHash = createServerCertificateHashes(options.singleHash);
  if (singleHash) {
    return {
      serverCertificateHashes: singleHash,
    };
  }

  return undefined;
}
