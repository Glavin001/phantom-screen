/**
 * Headed Chromium on $DISPLAY — use from this directory so `playwright` resolves.
 * CERT_HASH=64hex DISPLAY=:1 node live-desktop-demo.mjs
 */
import { chromium } from 'playwright';

const display = process.env.DISPLAY || ':0';
process.env.DISPLAY = display;

const cert = process.env.CERT_HASH;
if (!cert || !/^[a-f0-9]{64}$/i.test(cert)) {
  console.error('Set CERT_HASH to 64 hex chars: curl -s http://127.0.0.1:4444/health');
  process.exit(1);
}

const url = `http://127.0.0.1:4444/?serverUrl=https://127.0.0.1:4443&certHash=${cert}&autoconnect=1`;

const browser = await chromium.launch({
  headless: false,
  args: [
    '--origin-to-force-quic-on=127.0.0.1:4443',
    '--ignore-certificate-errors',
    '--enable-quic',
    '--disable-popup-blocking',
    '--window-position=200,100',
    '--window-size=1300,850',
  ],
});

const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 90_000 });

for (let i = 0; i < 120; i++) {
  const ok = await page.evaluate(() => {
    const root = document.getElementById('app')?.shadowRoot;
    if (!root) return false;
    const t = root.querySelector('[data-phantom-screen="status-text"]')?.textContent ?? '';
    return t.toLowerCase().includes('connected');
  });
  if (ok) break;
  await page.waitForTimeout(400);
}

await page.waitForTimeout(3000);
const out = process.env.SCREENSHOT_PATH || '/opt/cursor/artifacts/phantom-live-connected.png';
await page.screenshot({ path: out, fullPage: false });

await page.waitForTimeout(18_000);
await browser.close();
