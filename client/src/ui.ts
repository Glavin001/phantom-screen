/**
 * UI management: fullscreen, cursor handling, status display, resize.
 */

export interface UIElements {
  root: HTMLElement;
  container: HTMLElement;
  canvas: HTMLCanvasElement;
  statusDot: HTMLElement;
  statusText: HTMLElement;
  stats: HTMLElement;
  resolutionDisplay: HTMLElement;
  connectScreen: HTMLElement;
  errorMsg: HTMLElement;
  connectBtn: HTMLButtonElement;
  serverUrlInput: HTMLInputElement;
  certHashInput: HTMLInputElement;
  fullscreenBtn: HTMLButtonElement;
  pointerLockBtn: HTMLButtonElement;
  keyframeBtn: HTMLButtonElement;
  coherenceBtn: HTMLButtonElement;
  statusBar: HTMLElement;
  toolbar: HTMLElement;
}

function query<T extends Element>(root: ParentNode, name: string): T {
  const element = root.querySelector<T>(`[data-phantom-screen="${name}"]`);
  if (!element) {
    throw new Error(`Missing Phantom Screen UI element: ${name}`);
  }
  return element;
}

export function getUIElements(root: ParentNode): UIElements {
  return {
    root: query<HTMLElement>(root, 'root'),
    container: query<HTMLElement>(root, 'container'),
    canvas: query<HTMLCanvasElement>(root, 'desktop-canvas'),
    statusDot: query<HTMLElement>(root, 'status-dot'),
    statusText: query<HTMLElement>(root, 'status-text'),
    stats: query<HTMLElement>(root, 'stats'),
    resolutionDisplay: query<HTMLElement>(root, 'resolution-display'),
    connectScreen: query<HTMLElement>(root, 'connect-screen'),
    errorMsg: query<HTMLElement>(root, 'error-msg'),
    connectBtn: query<HTMLButtonElement>(root, 'connect-btn'),
    serverUrlInput: query<HTMLInputElement>(root, 'server-url'),
    certHashInput: query<HTMLInputElement>(root, 'cert-hash'),
    fullscreenBtn: query<HTMLButtonElement>(root, 'fullscreen-btn'),
    pointerLockBtn: query<HTMLButtonElement>(root, 'pointer-lock-btn'),
    keyframeBtn: query<HTMLButtonElement>(root, 'keyframe-btn'),
    coherenceBtn: query<HTMLButtonElement>(root, 'coherence-btn'),
    statusBar: query<HTMLElement>(root, 'status-bar'),
    toolbar: query<HTMLElement>(root, 'toolbar'),
  };
}

export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'error';

export function setConnectionState(ui: UIElements, state: ConnectionState, message?: string) {
  ui.statusDot.className = 'phantom-screen-status-dot ' + (state === 'connected' ? 'connected' : state === 'connecting' ? 'connecting' : state === 'error' ? 'error' : '');
  ui.statusText.textContent = message ?? state.charAt(0).toUpperCase() + state.slice(1);

  if (state === 'connected') {
    ui.connectScreen.classList.add('phantom-screen-hidden');
    ui.errorMsg.textContent = '';
    ui.connectBtn.disabled = false;
  } else if (state === 'disconnected' || state === 'error') {
    ui.connectScreen.classList.remove('phantom-screen-hidden');
    ui.connectBtn.disabled = false;
    if (message) {
      ui.errorMsg.textContent = message;
    }
  } else if (state === 'connecting') {
    ui.connectBtn.disabled = true;
    ui.errorMsg.textContent = '';
  }
}

export function updateStats(ui: UIElements, fps: number, frames: number, decodeTime: number) {
  ui.stats.textContent = `${fps} fps | ${frames} frames | decode: ${decodeTime.toFixed(1)}ms`;
}

export function updateResolution(ui: UIElements, width: number, height: number) {
  ui.resolutionDisplay.textContent = `${width}x${height}`;
}

/** Setup fullscreen toggle */
export function setupFullscreen(ui: UIElements) {
  const toggleFullscreen = () => {
    if (document.fullscreenElement) {
      void document.exitFullscreen();
    } else {
      void ui.root.requestFullscreen();
    }
  };

  // F11 shortcut is handled by the input capture (forwarded to remote)
  // Double-click on canvas to toggle fullscreen
  ui.fullscreenBtn.addEventListener('click', toggleFullscreen);
  ui.canvas.addEventListener('dblclick', toggleFullscreen);

  return () => {
    ui.fullscreenBtn.removeEventListener('click', toggleFullscreen);
    ui.canvas.removeEventListener('dblclick', toggleFullscreen);
  };
}

/** Setup pointer lock toggle */
export function setupPointerLock(ui: UIElements) {
  const onPointerLockChange = () => {
    if (document.pointerLockElement !== ui.canvas) {
      ui.pointerLockBtn.textContent = 'Lock Pointer';
    }
  };

  const togglePointerLock = () => {
    if (document.pointerLockElement === ui.canvas) {
      void document.exitPointerLock();
      ui.pointerLockBtn.textContent = 'Lock Pointer';
    } else {
      void ui.canvas.requestPointerLock();
      ui.pointerLockBtn.textContent = 'Unlock Pointer';
    }
  };

  ui.pointerLockBtn.addEventListener('click', togglePointerLock);
  document.addEventListener('pointerlockchange', onPointerLockChange);

  return () => {
    ui.pointerLockBtn.removeEventListener('click', togglePointerLock);
    document.removeEventListener('pointerlockchange', onPointerLockChange);
  };
}

/** Auto-hide toolbar and status bar after inactivity */
export function setupAutoHide(ui: UIElements) {
  let hideTimeout: number;

  function showUI() {
    ui.statusBar.classList.remove('phantom-screen-hidden');
    ui.toolbar.classList.remove('phantom-screen-hidden');
    clearTimeout(hideTimeout);
    hideTimeout = window.setTimeout(() => {
      ui.statusBar.classList.add('phantom-screen-hidden');
      ui.toolbar.classList.add('phantom-screen-hidden');
    }, 3000);
  }

  ui.root.addEventListener('mousemove', showUI);
  showUI();

  return () => {
    clearTimeout(hideTimeout);
    ui.root.removeEventListener('mousemove', showUI);
  };
}

/**
 * Get the scale factors to convert canvas client coordinates
 * to remote desktop coordinates.
 */
export function getCanvasScale(
  canvas: HTMLCanvasElement,
  remoteWidth: number,
  remoteHeight: number,
): { scaleX: number; scaleY: number; offsetX: number; offsetY: number } {
  const rect = canvas.getBoundingClientRect();
  const width = rect.width || 1;
  const height = rect.height || 1;
  return {
    scaleX: remoteWidth / width,
    scaleY: remoteHeight / height,
    offsetX: rect.left,
    offsetY: rect.top,
  };
}
