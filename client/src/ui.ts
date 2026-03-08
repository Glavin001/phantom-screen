/**
 * UI management: fullscreen, cursor handling, status display, resize.
 */

export interface UIElements {
  container: HTMLElement;
  canvas: HTMLCanvasElement;
  statusDot: HTMLElement;
  statusText: HTMLElement;
  stats: HTMLElement;
  resolutionDisplay: HTMLElement;
  connectScreen: HTMLElement;
  errorMsg: HTMLElement;
  connectBtn: HTMLButtonElement;
  fullscreenBtn: HTMLButtonElement;
  pointerLockBtn: HTMLButtonElement;
  keyframeBtn: HTMLButtonElement;
  statusBar: HTMLElement;
  toolbar: HTMLElement;
}

export function getUIElements(): UIElements {
  return {
    container: document.getElementById('container')!,
    canvas: document.getElementById('desktop-canvas') as HTMLCanvasElement,
    statusDot: document.getElementById('status-dot')!,
    statusText: document.getElementById('status-text')!,
    stats: document.getElementById('stats')!,
    resolutionDisplay: document.getElementById('resolution-display')!,
    connectScreen: document.getElementById('connect-screen')!,
    errorMsg: document.getElementById('error-msg')!,
    connectBtn: document.getElementById('connect-btn') as HTMLButtonElement,
    fullscreenBtn: document.getElementById('fullscreen-btn') as HTMLButtonElement,
    pointerLockBtn: document.getElementById('pointer-lock-btn') as HTMLButtonElement,
    keyframeBtn: document.getElementById('keyframe-btn') as HTMLButtonElement,
    statusBar: document.getElementById('status-bar')!,
    toolbar: document.getElementById('toolbar')!,
  };
}

export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'error';

export function setConnectionState(ui: UIElements, state: ConnectionState, message?: string) {
  ui.statusDot.className = 'status-dot ' + (state === 'connected' ? 'connected' : state === 'connecting' ? 'connecting' : state === 'error' ? 'error' : '');
  ui.statusText.textContent = message ?? state.charAt(0).toUpperCase() + state.slice(1);

  if (state === 'connected') {
    ui.connectScreen.classList.add('hidden');
  } else if (state === 'disconnected' || state === 'error') {
    ui.connectScreen.classList.remove('hidden');
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
  ui.fullscreenBtn.addEventListener('click', () => {
    if (document.fullscreenElement) {
      document.exitFullscreen();
    } else {
      ui.container.requestFullscreen();
    }
  });

  // F11 shortcut is handled by the input capture (forwarded to remote)
  // Double-click on canvas to toggle fullscreen
  ui.canvas.addEventListener('dblclick', () => {
    if (document.fullscreenElement) {
      document.exitFullscreen();
    } else {
      ui.container.requestFullscreen();
    }
  });
}

/** Setup pointer lock toggle */
export function setupPointerLock(ui: UIElements) {
  ui.pointerLockBtn.addEventListener('click', () => {
    if (document.pointerLockElement === ui.canvas) {
      document.exitPointerLock();
      ui.pointerLockBtn.textContent = 'Lock Pointer';
    } else {
      ui.canvas.requestPointerLock();
      ui.pointerLockBtn.textContent = 'Unlock Pointer';
    }
  });

  document.addEventListener('pointerlockchange', () => {
    if (document.pointerLockElement !== ui.canvas) {
      ui.pointerLockBtn.textContent = 'Lock Pointer';
    }
  });
}

/** Auto-hide toolbar and status bar after inactivity */
export function setupAutoHide(ui: UIElements) {
  let hideTimeout: number;

  function showUI() {
    ui.statusBar.classList.remove('hidden');
    ui.toolbar.classList.remove('hidden');
    clearTimeout(hideTimeout);
    hideTimeout = window.setTimeout(() => {
      ui.statusBar.classList.add('hidden');
      ui.toolbar.classList.add('hidden');
    }, 3000);
  }

  ui.container.addEventListener('mousemove', showUI);
  showUI();
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
  return {
    scaleX: remoteWidth / rect.width,
    scaleY: remoteHeight / rect.height,
    offsetX: rect.left,
    offsetY: rect.top,
  };
}
