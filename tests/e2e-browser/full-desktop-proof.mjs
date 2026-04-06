/**
 * End-to-end proof on $DISPLAY: Chromium + Docker Phantom Screen.
 * Saves PNGs + proof.json under OUT_DIR (default /opt/cursor/artifacts/phantom-proof-run).
 *
 * Required env: CERT_HASH (64 hex from curl $HTTP/health), DISPLAY=:1
 * Optional: PHANTOM_HTTP_ORIGIN, PHANTOM_WT_URL, OUT_DIR
 */
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'fs';
import { dirname } from 'path';

const display = process.env.DISPLAY || ':0';
process.env.DISPLAY = display;

const httpOrigin = process.env.PHANTOM_HTTP_ORIGIN ?? 'http://127.0.0.1:4444';
const wtUrl = process.env.PHANTOM_WT_URL ?? 'https://127.0.0.1:4443';
const cert = process.env.CERT_HASH;
const outDir = process.env.OUT_DIR ?? '/opt/cursor/artifacts/phantom-proof-run';

if (!cert || !/^[a-f0-9]{64}$/i.test(cert)) {
  console.error('CERT_HASH required (64 hex)');
  process.exit(1);
}

mkdirSync(outDir, { recursive: true });

let quicHint = '127.0.0.1:4443';
try {
  const u = new URL(wtUrl);
  if (u.port) quicHint = `${u.hostname}:${u.port}`;
} catch {
  /* ignore */
}

const url = `${httpOrigin}/?serverUrl=${encodeURIComponent(wtUrl)}&certHash=${cert}&autoconnect=1`;

const browser = await chromium.launch({
  headless: false,
  args: [
    `--origin-to-force-quic-on=${quicHint}`,
    '--ignore-certificate-errors',
    '--enable-quic',
    '--disable-popup-blocking',
    '--start-maximized',
  ],
});

const context = await browser.newContext({
  viewport: { width: 1920, height: 1080 },
});
const page = await context.newPage();

await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 90_000 });

let proof = { url, steps: [] };

for (let i = 0; i < 150; i++) {
  const snap = await page.evaluate(() => {
    const root = document.getElementById('app')?.shadowRoot;
    if (!root) return { error: 'no-shadow' };
    const status = root.querySelector('[data-phantom-screen="status-text"]')?.textContent?.trim() ?? '';
    const stats = root.querySelector('[data-phantom-screen="stats"]')?.textContent?.trim() ?? '';
    const err = root.querySelector('[data-phantom-screen="error-msg"]')?.textContent?.trim() ?? '';
    const connectHidden = root
      .querySelector('[data-phantom-screen="connect-screen"]')
      ?.classList.contains('phantom-screen-hidden');
    return { status, stats, err, connectHidden };
  });
  proof.steps.push({ t: i * 200, ...snap });
  if (snap.status.toLowerCase().includes('connected') && /\d+\s*fps/i.test(snap.stats)) break;
  await page.waitForTimeout(200);
}

proof.final = proof.steps[proof.steps.length - 1];

await page.waitForTimeout(2500);

await page.screenshot({ path: `${outDir}/01-full-viewport-connected.png`, fullPage: true });

const barBox = await page.evaluate(() => {
  const root = document.getElementById('app')?.shadowRoot;
  const bar = root?.querySelector('[data-phantom-screen="status-bar"]');
  if (!bar) return null;
  const r = bar.getBoundingClientRect();
  return { x: r.x, y: r.y, width: r.width, height: r.height };
});
if (barBox && barBox.width > 0) {
  await page.screenshot({
    path: `${outDir}/02-status-bar-closeup.png`,
    clip: {
      x: barBox.x,
      y: barBox.y,
      width: Math.min(barBox.width, 1920),
      height: Math.min(barBox.height + 4, 200),
    },
  });
}

const canvasBox = await page.evaluate(() => {
  const root = document.getElementById('app')?.shadowRoot;
  const c = root?.querySelector('[data-phantom-screen="desktop-canvas"]');
  if (!c) return null;
  const r = c.getBoundingClientRect();
  return { x: r.x, y: r.y, width: r.width, height: r.height };
});
if (canvasBox && canvasBox.width > 0) {
  await page.screenshot({
    path: `${outDir}/03-remote-desktop-canvas.png`,
    clip: {
      x: canvasBox.x,
      y: canvasBox.y,
      width: Math.min(canvasBox.width, 1920),
      height: Math.min(canvasBox.height, 1080),
    },
  });
}

writeFileSync(`${outDir}/proof.json`, JSON.stringify(proof, null, 2));

await browser.close();
console.log('OK', outDir);
