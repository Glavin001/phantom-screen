/**
 * Input capture and binary serialization for the Phantom Screen input protocol.
 *
 * Protocol format:
 *   Mouse Move:    [0x01] [x: u16 BE] [y: u16 BE]                    = 5 bytes
 *   Mouse Button:  [0x02] [button: u8] [pressed: u8]                  = 3 bytes
 *   Mouse Scroll:  [0x03] [dx: i16 BE] [dy: i16 BE]                  = 5 bytes
 *   Key Event:     [0x10] [code_len: u8] [code: utf8] [pressed: u8]   = variable
 *   Clipboard:     [0x20] [length: u32 BE] [utf8 data...]             = variable
 *   Control:       [0x30] [subtype: u8] [payload...]                  = variable
 */

export type InputSender = (data: Uint8Array) => void;

/** Serialize a mouse move event */
export function encodeMouseMove(x: number, y: number): Uint8Array {
  const buf = new Uint8Array(5);
  const view = new DataView(buf.buffer);
  buf[0] = 0x01;
  view.setUint16(1, Math.round(x), false);
  view.setUint16(3, Math.round(y), false);
  return buf;
}

/** Serialize a mouse button event */
export function encodeMouseButton(button: number, pressed: boolean): Uint8Array {
  const buf = new Uint8Array(3);
  // Map DOM button numbers to X11 button numbers
  // DOM: 0=left, 1=middle, 2=right, 3=back, 4=forward
  // X11: 1=left, 2=middle, 3=right, 8=back, 9=forward
  const x11Button = [1, 2, 3, 8, 9][button] ?? (button + 1);
  buf[0] = 0x02;
  buf[1] = x11Button;
  buf[2] = pressed ? 1 : 0;
  return buf;
}

/** Serialize a mouse scroll event */
export function encodeMouseScroll(dx: number, dy: number): Uint8Array {
  const buf = new Uint8Array(5);
  const view = new DataView(buf.buffer);
  buf[0] = 0x03;
  // Normalize scroll deltas to discrete steps
  const stepX = Math.sign(dx) * Math.min(Math.ceil(Math.abs(dx) / 120), 10);
  const stepY = Math.sign(dy) * Math.min(Math.ceil(Math.abs(dy) / 120), 10);
  view.setInt16(1, stepX, false);
  view.setInt16(3, stepY, false);
  return buf;
}

/** Serialize a key event */
export function encodeKeyEvent(code: string, pressed: boolean): Uint8Array {
  const encoder = new TextEncoder();
  const codeBytes = encoder.encode(code);
  if (codeBytes.length > 255) return new Uint8Array(0);

  const buf = new Uint8Array(2 + codeBytes.length + 1);
  buf[0] = 0x10;
  buf[1] = codeBytes.length;
  buf.set(codeBytes, 2);
  buf[2 + codeBytes.length] = pressed ? 1 : 0;
  return buf;
}

/** Serialize clipboard text */
export function encodeClipboard(text: string): Uint8Array {
  const encoder = new TextEncoder();
  const textBytes = encoder.encode(text);
  const buf = new Uint8Array(5 + textBytes.length);
  const view = new DataView(buf.buffer);
  buf[0] = 0x20;
  view.setUint32(1, textBytes.length, false);
  buf.set(textBytes, 5);
  return buf;
}

/** Serialize a keyframe request */
export function encodeKeyframeRequest(): Uint8Array {
  return new Uint8Array([0x30, 0x01]);
}

/** Serialize a bitrate change request */
export function encodeBitrateRequest(kbps: number): Uint8Array {
  const buf = new Uint8Array(6);
  const view = new DataView(buf.buffer);
  buf[0] = 0x30;
  buf[1] = 0x02;
  view.setUint32(2, kbps, false);
  return buf;
}

/** Serialize a resolution change request */
export function encodeResolutionRequest(width: number, height: number): Uint8Array {
  const buf = new Uint8Array(6);
  const view = new DataView(buf.buffer);
  buf[0] = 0x30;
  buf[1] = 0x03;
  view.setUint16(2, width, false);
  view.setUint16(4, height, false);
  return buf;
}

// ── Coherence mode protocol (0x40 prefix) ────────────────────────────

/** Enable coherence mode */
export function encodeEnableCoherence(): Uint8Array {
  return new Uint8Array([0x40, 0x01]);
}

/** Disable coherence mode */
export function encodeDisableCoherence(): Uint8Array {
  return new Uint8Array([0x40, 0x02]);
}

/** Subscribe to a window's video stream */
export function encodeSubscribeWindow(windowId: number): Uint8Array {
  const buf = new Uint8Array(6);
  const view = new DataView(buf.buffer);
  buf[0] = 0x40;
  buf[1] = 0x03;
  view.setUint32(2, windowId, false);
  return buf;
}

/** Unsubscribe from a window's video stream */
export function encodeUnsubscribeWindow(windowId: number): Uint8Array {
  const buf = new Uint8Array(6);
  const view = new DataView(buf.buffer);
  buf[0] = 0x40;
  buf[1] = 0x04;
  view.setUint32(2, windowId, false);
  return buf;
}

/** Resize a remote window */
export function encodeResizeWindow(windowId: number, width: number, height: number): Uint8Array {
  const buf = new Uint8Array(10);
  const view = new DataView(buf.buffer);
  buf[0] = 0x40;
  buf[1] = 0x05;
  view.setUint32(2, windowId, false);
  view.setUint16(6, width, false);
  view.setUint16(8, height, false);
  return buf;
}

/** Focus/raise a remote window */
export function encodeFocusWindow(windowId: number): Uint8Array {
  const buf = new Uint8Array(6);
  const view = new DataView(buf.buffer);
  buf[0] = 0x40;
  buf[1] = 0x06;
  view.setUint32(2, windowId, false);
  return buf;
}

/** Close a remote window */
export function encodeCloseWindow(windowId: number): Uint8Array {
  const buf = new Uint8Array(6);
  const view = new DataView(buf.buffer);
  buf[0] = 0x40;
  buf[1] = 0x07;
  view.setUint32(2, windowId, false);
  return buf;
}

/** Launch an application on the remote desktop */
export function encodeLaunchApp(command: string): Uint8Array {
  const encoder = new TextEncoder();
  const cmdBytes = encoder.encode(command);
  const buf = new Uint8Array(4 + cmdBytes.length);
  const view = new DataView(buf.buffer);
  buf[0] = 0x40;
  buf[1] = 0x08;
  view.setUint16(2, cmdBytes.length, false);
  buf.set(cmdBytes, 4);
  return buf;
}

/**
 * Attach input event listeners to the canvas element.
 * Returns a cleanup function to remove listeners.
 */
export function attachInputListeners(
  canvas: HTMLCanvasElement,
  send: InputSender,
  getScale: () => { scaleX: number; scaleY: number; offsetX: number; offsetY: number },
): () => void {
  // Throttle mouse moves to ~60 per second
  let lastMouseMove = 0;
  const MOUSE_THROTTLE_MS = 16;

  function onMouseMove(e: MouseEvent) {
    const now = performance.now();
    if (now - lastMouseMove < MOUSE_THROTTLE_MS) return;
    lastMouseMove = now;

    const { scaleX, scaleY, offsetX, offsetY } = getScale();
    const x = (e.clientX - offsetX) * scaleX;
    const y = (e.clientY - offsetY) * scaleY;
    if (x >= 0 && y >= 0) {
      send(encodeMouseMove(x, y));
    }
  }

  function sendMousePosition(e: MouseEvent) {
    const { scaleX, scaleY, offsetX, offsetY } = getScale();
    const x = (e.clientX - offsetX) * scaleX;
    const y = (e.clientY - offsetY) * scaleY;
    if (x >= 0 && y >= 0) {
      send(encodeMouseMove(x, y));
    }
  }

  function onMouseDown(e: MouseEvent) {
    e.preventDefault();
    canvas.focus();
    sendMousePosition(e);
    send(encodeMouseButton(e.button, true));
  }

  function onMouseUp(e: MouseEvent) {
    e.preventDefault();
    sendMousePosition(e);
    send(encodeMouseButton(e.button, false));
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    send(encodeMouseScroll(e.deltaX, e.deltaY));
  }

  function onKeyDown(e: KeyboardEvent) {
    e.preventDefault();
    e.stopPropagation();
    send(encodeKeyEvent(e.code, true));
  }

  function onKeyUp(e: KeyboardEvent) {
    e.preventDefault();
    e.stopPropagation();
    send(encodeKeyEvent(e.code, false));
  }

  function onContextMenu(e: Event) {
    e.preventDefault();
  }

  function onPaste(e: ClipboardEvent) {
    e.preventDefault();
    const text = e.clipboardData?.getData('text/plain');
    if (text) {
      send(encodeClipboard(text));
    }
  }

  canvas.addEventListener('mousemove', onMouseMove);
  canvas.addEventListener('mousedown', onMouseDown);
  canvas.addEventListener('mouseup', onMouseUp);
  canvas.addEventListener('wheel', onWheel, { passive: false });
  canvas.addEventListener('keydown', onKeyDown);
  canvas.addEventListener('keyup', onKeyUp);
  canvas.addEventListener('contextmenu', onContextMenu);
  canvas.addEventListener('paste', onPaste);

  // Return cleanup function
  return () => {
    canvas.removeEventListener('mousemove', onMouseMove);
    canvas.removeEventListener('mousedown', onMouseDown);
    canvas.removeEventListener('mouseup', onMouseUp);
    canvas.removeEventListener('wheel', onWheel);
    canvas.removeEventListener('keydown', onKeyDown);
    canvas.removeEventListener('keyup', onKeyUp);
    canvas.removeEventListener('contextmenu', onContextMenu);
    canvas.removeEventListener('paste', onPaste);
  };
}
