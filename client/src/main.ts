/**
 * Phantom Screen — Web Client
 *
 * Connects to the Phantom Screen server via WebTransport,
 * receives H.264 video frames, decodes with WebCodecs,
 * renders to canvas, and sends input events back.
 */

import { attachInputListeners, type InputSender } from './input';
import { ClipboardSync } from './clipboard';
import { ControlManager } from './control';
import {
  getUIElements,
  setConnectionState,
  setupFullscreen,
  setupPointerLock,
  setupAutoHide,
  getCanvasScale,
} from './ui';

const ui = getUIElements();

// Setup UI handlers
setupFullscreen(ui);
setupPointerLock(ui);
setupAutoHide(ui);

// Connection state
let transport: WebTransport | null = null;
let inputWriter: WritableStreamDefaultWriter<Uint8Array> | null = null;
let decoder: VideoDecoder | null = null;
let controlManager: ControlManager | null = null;
let cleanupInput: (() => void) | null = null;
let clipboardSync = new ClipboardSync();

// Canvas rendering context
const ctx = ui.canvas.getContext('2d')!;

// Connect button handler
ui.connectBtn.addEventListener('click', () => {
  const urlInput = document.getElementById('server-url') as HTMLInputElement;
  connect(urlInput.value.trim());
});

// Enter key in URL input
(document.getElementById('server-url') as HTMLInputElement).addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    ui.connectBtn.click();
  }
});

async function connect(serverUrl: string) {
  // Clean up previous connection
  disconnect();

  if (!serverUrl) {
    setConnectionState(ui, 'error', 'Please enter a server URL');
    return;
  }

  setConnectionState(ui, 'connecting', 'Connecting...');

  try {
    // Create WebTransport connection
    transport = new WebTransport(serverUrl, {
      // For self-signed certs in development, users need to launch Chrome with
      // --ignore-certificate-errors-spiffe-list or use chrome://flags
    });

    await transport.ready;
    setConnectionState(ui, 'connected', 'Connected');

    // Setup input sender via bidirectional stream
    const biStream = await transport.createBidirectionalStream();
    const writer = biStream.writable.getWriter();
    inputWriter = writer;

    const send: InputSender = (data: Uint8Array) => {
      if (inputWriter) {
        inputWriter.write(data).catch(() => {
          // Stream closed, ignore
        });
      }
    };

    // Setup control manager
    controlManager = new ControlManager(send, ui);

    // Setup WebCodecs video decoder
    setupDecoder();

    // Attach input listeners
    cleanupInput = attachInputListeners(
      ui.canvas,
      send,
      () => getCanvasScale(
        ui.canvas,
        controlManager!.getRemoteWidth(),
        controlManager!.getRemoteHeight(),
      ),
    );

    // Focus canvas
    ui.canvas.focus();

    // Read incoming video frames from unidirectional streams
    readVideoStreams(transport);

    // Read clipboard updates from the bidirectional stream reader
    readInputResponses(biStream.readable.getReader());

    // Handle connection close
    transport.closed
      .then(() => {
        setConnectionState(ui, 'disconnected', 'Connection closed');
      })
      .catch((e: Error) => {
        setConnectionState(ui, 'error', `Connection lost: ${e.message}`);
      });

  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    setConnectionState(ui, 'error', `Failed to connect: ${msg}`);
  }
}

function disconnect() {
  cleanupInput?.();
  cleanupInput = null;
  controlManager?.destroy();
  controlManager = null;
  decoder?.close();
  decoder = null;
  inputWriter = null;
  transport?.close();
  transport = null;
}

function setupDecoder() {
  decoder = new VideoDecoder({
    output: (frame: VideoFrame) => {
      // Draw frame to canvas
      const startDraw = performance.now();

      // Update canvas size if needed
      if (ui.canvas.width !== frame.displayWidth || ui.canvas.height !== frame.displayHeight) {
        ui.canvas.width = frame.displayWidth;
        ui.canvas.height = frame.displayHeight;
        controlManager?.setRemoteResolution(frame.displayWidth, frame.displayHeight);
      }

      ctx.drawImage(frame, 0, 0);
      frame.close();

      const drawTime = performance.now() - startDraw;
      controlManager?.recordFrame(drawTime);
    },
    error: (e: DOMException) => {
      console.error('Decoder error:', e);
      // Request a keyframe to recover
      controlManager?.requestKeyframe();
    },
  });

  decoder.configure({
    codec: 'avc1.42001f', // H.264 Baseline Level 3.1
    hardwareAcceleration: 'prefer-hardware',
    optimizeForLatency: true,
  });
}

async function readVideoStreams(wt: WebTransport) {
  const reader = wt.incomingUnidirectionalStreams.getReader();

  try {
    while (true) {
      const { value: stream, done } = await reader.read();
      if (done) break;

      // Process each video frame stream
      processVideoStream(stream).catch((e) => {
        console.warn('Video stream error:', e);
      });
    }
  } catch (e) {
    console.warn('Incoming streams reader closed:', e);
  }
}

async function processVideoStream(stream: ReadableStream<Uint8Array>) {
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let totalLength = 0;

  try {
    // Read all data from the stream (each stream = one frame)
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      chunks.push(value);
      totalLength += value.length;
    }
  } catch {
    return; // Stream closed/errored
  }

  if (totalLength < 13) return; // Too short for header

  // Combine chunks
  const data = new Uint8Array(totalLength);
  let offset = 0;
  for (const chunk of chunks) {
    data.set(chunk, offset);
    offset += chunk.length;
  }

  // Parse frame header:
  // [flags: u8] [pts: u64 BE] [length: u32 BE] [H.264 data...]
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const flags = data[0];
  const isKeyframe = (flags & 0x01) !== 0;
  // Read pts as BigInt64 but convert to number (microseconds)
  const ptsHigh = view.getUint32(1, false);
  const ptsLow = view.getUint32(5, false);
  const pts = ptsHigh * 0x100000000 + ptsLow;
  const payloadLength = view.getUint32(9, false);

  if (data.length < 13 + payloadLength) return;

  const h264Data = data.slice(13, 13 + payloadLength);

  if (!decoder || decoder.state === 'closed') return;

  try {
    const chunk = new EncodedVideoChunk({
      type: isKeyframe ? 'key' : 'delta',
      timestamp: pts / 1000, // ns to us
      data: h264Data,
    });

    decoder.decode(chunk);
  } catch (e) {
    console.warn('Decode error:', e);
    controlManager?.requestKeyframe();
  }
}

async function readInputResponses(reader: ReadableStreamDefaultReader<Uint8Array>) {
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;

      // Parse server-to-client messages (clipboard updates, etc.)
      if (value && value.length > 0) {
        if (value[0] === 0x20 && value.length >= 5) {
          // Clipboard update from server
          const view = new DataView(value.buffer, value.byteOffset, value.byteLength);
          const textLen = view.getUint32(1, false);
          if (value.length >= 5 + textLen) {
            const text = new TextDecoder().decode(value.slice(5, 5 + textLen));
            clipboardSync.receiveClipboard(text);
          }
        }
      }
    }
  } catch {
    // Stream closed
  }
}
