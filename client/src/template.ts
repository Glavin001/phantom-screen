export const DEFAULT_SERVER_URL = 'https://127.0.0.1:4443';

const DEFAULT_TITLE = 'Phantom Screen';
const DEFAULT_SUBTITLE = 'Remote desktop via WebTransport';

const styles = `
  :host,
  .phantom-screen-root {
    display: block;
    width: 100%;
    height: 100%;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  }

  * {
    box-sizing: border-box;
  }

  .phantom-screen-root {
    position: relative;
    min-height: 360px;
    background: #0f0f23;
    overflow: hidden;
  }

  .phantom-screen-container {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: #0f0f23;
  }

  .phantom-screen-canvas {
    background: #000;
    cursor: none;
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }

  .phantom-screen-status-bar {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 32px;
    background: rgba(15, 15, 35, 0.9);
    backdrop-filter: blur(8px);
    display: flex;
    align-items: center;
    padding: 0 12px;
    font-size: 12px;
    z-index: 100;
    transition: opacity 0.3s;
    gap: 16px;
  }

  .phantom-screen-toolbar {
    position: absolute;
    bottom: 16px;
    left: 50%;
    transform: translateX(-50%);
    background: rgba(15, 15, 35, 0.9);
    backdrop-filter: blur(8px);
    border-radius: 8px;
    padding: 6px 12px;
    display: flex;
    gap: 8px;
    z-index: 100;
    transition: opacity 0.3s;
  }

  .phantom-screen-hidden {
    opacity: 0;
    pointer-events: none;
  }

  .phantom-screen-status-item {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .phantom-screen-status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #555;
  }

  .phantom-screen-status-dot.connected {
    background: #4caf50;
  }

  .phantom-screen-status-dot.connecting {
    background: #ff9800;
    animation: phantom-screen-pulse 1s infinite;
  }

  .phantom-screen-status-dot.error {
    background: #f44336;
  }

  @keyframes phantom-screen-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
  }

  .phantom-screen-toolbar-btn,
  .phantom-screen-connect-btn,
  .phantom-screen-input {
    font: inherit;
  }

  .phantom-screen-toolbar-btn,
  .phantom-screen-connect-btn {
    border-radius: 6px;
    cursor: pointer;
    transition: background 0.2s;
  }

  .phantom-screen-toolbar-btn {
    background: rgba(255, 255, 255, 0.1);
    border: 1px solid rgba(255, 255, 255, 0.2);
    color: #e0e0e0;
    padding: 6px 12px;
    font-size: 12px;
  }

  .phantom-screen-toolbar-btn:hover {
    background: rgba(255, 255, 255, 0.2);
  }

  .phantom-screen-connect-screen {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 16px;
    padding: 24px;
    background: rgba(15, 15, 35, 0.96);
    z-index: 200;
  }

  .phantom-screen-connect-screen.phantom-screen-hidden {
    display: none;
  }

  .phantom-screen-heading {
    text-align: center;
  }

  .phantom-screen-heading h1 {
    margin: 0 0 8px;
    font-size: 32px;
    font-weight: 300;
    color: #fff;
  }

  .phantom-screen-heading p {
    margin: 0;
    color: #888;
  }

  .phantom-screen-connect-form {
    width: min(440px, 100%);
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .phantom-screen-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    color: #c8c8d3;
    font-size: 13px;
  }

  .phantom-screen-input {
    width: 100%;
    background: rgba(255, 255, 255, 0.1);
    border: 1px solid rgba(255, 255, 255, 0.2);
    color: #fff;
    padding: 10px 14px;
    border-radius: 6px;
    outline: none;
  }

  .phantom-screen-input:focus {
    border-color: rgba(100, 150, 255, 0.5);
  }

  .phantom-screen-connect-btn {
    background: #4a6cf7;
    border: none;
    color: white;
    padding: 10px 24px;
    font-size: 14px;
  }

  .phantom-screen-connect-btn:hover {
    background: #3a5ce7;
  }

  .phantom-screen-connect-btn:disabled {
    background: #333;
    cursor: not-allowed;
  }

  .phantom-screen-help {
    color: #8e8ea0;
    font-size: 12px;
    line-height: 1.4;
  }

  .phantom-screen-help code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  }

  .phantom-screen-error {
    min-height: 18px;
    color: #f44336;
    font-size: 13px;
  }

  .phantom-screen-stats {
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    font-size: 11px;
    color: #888;
  }

  .phantom-screen-spacer {
    flex: 1;
  }
`;

function escapeAttribute(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('"', '&quot;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;');
}

export interface TemplateOptions {
  title?: string;
  subtitle?: string;
  serverUrl?: string;
  certificateHash?: string;
}

export function renderTemplate(root: ShadowRoot | HTMLElement, options: TemplateOptions = {}): void {
  const title = options.title ?? DEFAULT_TITLE;
  const subtitle = options.subtitle ?? DEFAULT_SUBTITLE;
  const serverUrl = options.serverUrl ?? DEFAULT_SERVER_URL;
  const certificateHash = options.certificateHash ?? '';

  root.innerHTML = `
    <style>${styles}</style>
    <div class="phantom-screen-root" data-phantom-screen="root">
      <div class="phantom-screen-connect-screen" data-phantom-screen="connect-screen">
        <div class="phantom-screen-heading">
          <h1>${title}</h1>
          <p>${subtitle}</p>
        </div>
        <div class="phantom-screen-connect-form">
          <label class="phantom-screen-field">
            <span>Server URL</span>
            <input
              class="phantom-screen-input"
              data-phantom-screen="server-url"
              type="text"
              placeholder="${escapeAttribute(DEFAULT_SERVER_URL)}"
              value="${escapeAttribute(serverUrl)}"
            />
          </label>
          <label class="phantom-screen-field">
            <span>Server certificate SHA-256 hash</span>
            <input
              class="phantom-screen-input"
              data-phantom-screen="cert-hash"
              type="text"
              placeholder="optional in production, required for self-signed WebTransport"
              value="${escapeAttribute(certificateHash)}"
            />
          </label>
          <button class="phantom-screen-connect-btn" data-phantom-screen="connect-btn">Connect</button>
          <div class="phantom-screen-help">
            For local self-signed servers, pass the SHA-256 cert hash in hex or base64.
            The standalone page also accepts <code>?serverUrl=...</code>, <code>?certHash=...</code>,
            and <code>?autoconnect=1</code>.
          </div>
          <div class="phantom-screen-error" data-phantom-screen="error-msg"></div>
        </div>
      </div>

      <div class="phantom-screen-container" data-phantom-screen="container">
        <canvas class="phantom-screen-canvas" data-phantom-screen="desktop-canvas" tabindex="0"></canvas>
      </div>

      <div class="phantom-screen-status-bar" data-phantom-screen="status-bar">
        <div class="phantom-screen-status-item">
          <div class="phantom-screen-status-dot" data-phantom-screen="status-dot"></div>
          <span data-phantom-screen="status-text">Disconnected</span>
        </div>
        <div class="phantom-screen-status-item phantom-screen-stats" data-phantom-screen="stats"></div>
        <div class="phantom-screen-spacer"></div>
        <div class="phantom-screen-status-item">
          <span data-phantom-screen="resolution-display"></span>
        </div>
      </div>

      <div class="phantom-screen-toolbar" data-phantom-screen="toolbar">
        <button class="phantom-screen-toolbar-btn" data-phantom-screen="fullscreen-btn" title="Toggle fullscreen">
          Fullscreen
        </button>
        <button class="phantom-screen-toolbar-btn" data-phantom-screen="pointer-lock-btn" title="Toggle pointer lock">
          Lock Pointer
        </button>
        <button class="phantom-screen-toolbar-btn" data-phantom-screen="keyframe-btn" title="Request a keyframe">
          Refresh
        </button>
      </div>
    </div>
  `;
}
