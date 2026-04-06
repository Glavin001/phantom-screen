/**
 * Two screenshots on $DISPLAY: (1) connect form with full cert hash visible in textarea
 * (2) connected stream. Requires Playwright from this directory.
 *
 * PHANTOM_HTTP_ORIGIN PHANTOM_WT_URL DISPLAY CERT_HASH from /health
 */
import { chromium } from 'playwright';

const display = process.env.DISPLAY || ':0';
process.env.DISPLAY = display;

const httpOrigin = process.env.PHANTOM_HTTP_ORIGIN ?? 'http://127.0.0.1:4444';
const wtUrl = process.env.PHANTOM_WT_URL ?? 'https://127.0.0.1:4443';
const cert = process.env.CERT_HASH;
if (!cert || !/^[a-f0-9]{64}$/i.test(cert)) {
  console.error('CERT_HASH required (64 hex from /health)');
  process.exit(1);
}

let quicHint = '127.0.0.1:4443';
try {
  const u = new URL(wtUrl);
  if (u.port) quicHint = `${u.hostname}:${u.port}`;
} catch {
  /* ignore */
}

const urlShowHash = `${httpOrigin}/?serverUrl=${encodeURIComponent(wtUrl)}&certHash=${cert}&autoconnect=0`;
const urlAuto = `${httpOrigin}/?serverUrl=${encodeURIComponent(wtUrl)}&certHash=${cert}&autoconnect=1`;

const outConnect = process.env.OUT_CONNECT ?? '/opt/cursor/artifacts/phantom-proof-connect-form.png';
const outStream = process.env.OUT_STREAM ?? '/opt/cursor/artifacts/phantom-proof-connected-stream.png';

const browser = await chromium.launch({
  headless: false,
  args: [
    `--origin-to-force-quic-on=${quicHint}`,
    '--ignore-certificate-errors',
    '--enable-quic',
    '--disable-popup-blocking',
    '--window-position=80,60',
    '--window-size=1400,900',
  ],
});

const page = await browser.newPage({ viewport: { width: 1360, height: 860 } });
await page.goto(urlShowHash, { waitUntil: 'networkidle', timeout: 60_000 });
await page.waitForTimeout(800);
await page.screenshot({ path: outConnect, fullPage: true });

await page.goto(urlAuto, { waitUntil: 'domcontentloaded', timeout: 60_000 });
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
await page.waitForTimeout(3500);
await page.screenshot({ path: outStream, fullPage: false });

await browser.close();
console.log('wrote', outConnect, outStream);
