import { chromium } from 'playwright';

const display = process.env.DISPLAY || ':0';
process.env.DISPLAY = display;

const cert = process.env.CERT_HASH;
if (!cert) {
  console.error('Set CERT_HASH to the server cert SHA-256 (hex)');
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
    '--window-position=240,120',
    '--window-size=1280,820',
  ],
});

const context = await browser.newContext({
  viewport: { width: 1260, height: 780 },
});
const page = await context.newPage();
await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 90_000 });

for (let i = 0; i < 180; i++) {
  const ok = await page.evaluate(() => {
    const root = document.getElementById('app')?.shadowRoot;
    if (!root) return false;
    const t = root.querySelector('[data-phantom-screen="status-text"]')?.textContent ?? '';
    const stats = root.querySelector('[data-phantom-screen="stats"]')?.textContent ?? '';
    return t.toLowerCase().includes('connected') && /\d+\s*frames/i.test(stats);
  });
  if (ok) break;
  await page.waitForTimeout(500);
}

await page.waitForTimeout(18_000);
await browser.close();
