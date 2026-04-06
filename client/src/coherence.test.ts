// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { CoherenceController, type WindowInfo } from './coherence';
import * as input from './input';

function encodeWindowRecord(info: WindowInfo): Uint8Array {
  const title = new TextEncoder().encode(info.title);
  const appClass = new TextEncoder().encode(info.appClass);
  const buf = new Uint8Array(13 + 2 + title.length + 2 + appClass.length);
  const view = new DataView(buf.buffer);
  let o = 0;
  view.setUint32(o, info.windowId, false);
  o += 4;
  view.setInt16(o, info.x, false);
  o += 2;
  view.setInt16(o, info.y, false);
  o += 2;
  view.setUint16(o, info.width, false);
  o += 2;
  view.setUint16(o, info.height, false);
  o += 2;
  buf[o] = info.visible ? 1 : 0;
  o += 1;
  view.setUint16(o, title.length, false);
  o += 2;
  buf.set(title, o);
  o += title.length;
  view.setUint16(o, appClass.length, false);
  o += 2;
  buf.set(appClass, o);
  return buf;
}

/** Binary snapshot as sent by the server (0x40 0x01). */
function encodeSnapshot(windows: WindowInfo[]): Uint8Array {
  const records = windows.map(encodeWindowRecord);
  const payloadLen = records.reduce((n, r) => n + r.length, 0);
  const out = new Uint8Array(4 + payloadLen);
  out[0] = 0x40;
  out[1] = 0x01;
  new DataView(out.buffer).setUint16(2, windows.length, false);
  let o = 4;
  for (const r of records) {
    out.set(r, o);
    o += r.length;
  }
  return out;
}

describe('CoherenceController', () => {
  let send: ReturnType<typeof vi.fn<(data: Uint8Array) => void>>;
  let onStreamError: ReturnType<typeof vi.fn<(msg: string) => void>>;
  let origGetContext: typeof HTMLCanvasElement.prototype.getContext;

  const sampleWindow: WindowInfo = {
    windowId: 42,
    title: 'Test',
    x: 0,
    y: 0,
    width: 640,
    height: 480,
    visible: true,
    appClass: 'TestApp',
  };

  beforeEach(() => {
    send = vi.fn();
    onStreamError = vi.fn();
    vi.stubGlobal(
      'VideoDecoder',
      class {
        state: VideoDecoder['state'] = 'configured';
        constructor() {}
        configure(_config: VideoDecoderConfig): void {}
        decode(_chunk: EncodedVideoChunk): void {}
        close(): void {
          this.state = 'closed';
        }
      },
    );
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe(): void {}
        unobserve(): void {}
        disconnect(): void {}
      },
    );
    origGetContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (
      this: HTMLCanvasElement,
      contextId: string,
      ...args: unknown[]
    ) {
      if (contextId === '2d') {
        return {
          drawImage: vi.fn(),
          getImageData: vi.fn(),
        } as unknown as CanvasRenderingContext2D;
      }
      return origGetContext.apply(this, [contextId, ...args] as Parameters<HTMLCanvasElement['getContext']>);
    } as typeof HTMLCanvasElement.prototype.getContext;
  });

  afterEach(() => {
    HTMLCanvasElement.prototype.getContext = origGetContext;
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('reports a clear error when pop-out is blocked', () => {
    vi.stubGlobal('open', vi.fn(() => null));

    const ctrl = new CoherenceController(send, { onStreamError }, 'prefer-software');
    ctrl.enableCoherenceMode();
    ctrl.handleWindowEventData(encodeSnapshot([sampleWindow]));

    ctrl.openWindowAsPopup(42);

    expect(onStreamError).toHaveBeenCalled();
    const last = onStreamError.mock.calls[onStreamError.mock.calls.length - 1][0];
    expect(String(last)).toMatch(/pop-up|Pop-out|blocked/i);
  });

  it('clears the stream error banner when opening a new stream', () => {
    const ctrl = new CoherenceController(send, { onStreamError }, 'prefer-software');
    ctrl.handleWindowEventData(encodeSnapshot([sampleWindow]));

    const fakeDoc = document.implementation.createHTMLDocument('');
    const fakeWin = {
      document: fakeDoc,
      innerWidth: 640,
      innerHeight: 480,
      closed: false,
      focus: vi.fn(),
      addEventListener: vi.fn(),
      postMessage: vi.fn(),
      close: vi.fn(),
    } as unknown as Window;

    vi.stubGlobal('open', vi.fn(() => fakeWin));

    ctrl.openWindowAsPopup(42);
    expect(onStreamError).toHaveBeenCalledWith('');

    ctrl.openWindowAsPopup(42);
    expect(onStreamError).toHaveBeenCalledWith('');
  });

  it('sends subscribe when opening inline stream', () => {
    const spy = vi.spyOn(input, 'encodeSubscribeWindow');

    const ctrl = new CoherenceController(send, {}, 'prefer-software');
    const parent = document.createElement('div');
    ctrl.setInlineParent(parent);
    ctrl.handleWindowEventData(encodeSnapshot([sampleWindow]));

    ctrl.openWindowPopup(42);

    expect(spy).toHaveBeenCalledWith(42);
    expect(send).toHaveBeenCalled();
    const subscribeCall = send.mock.calls.map((c) => c[0]).find((buf) => buf[0] === 0x40 && buf[1] === 0x03);
    expect(subscribeCall).toBeDefined();
    expect(new DataView(subscribeCall!.buffer, subscribeCall!.byteOffset, 6).getUint32(2, false)).toBe(42);

    spy.mockRestore();
  });
});
