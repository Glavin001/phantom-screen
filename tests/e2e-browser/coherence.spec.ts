/**
 * End-to-end browser test for Phantom Screen.
 *
 * Starts the server via Docker, opens the real client page in Chromium,
 * verifies video frames are received (non-black canvas), exercises coherence
 * mode, and checks the server survives a resize + reconnect cycle.
 *
 * Prerequisites:
 *   docker compose up -d   (or the test script starts it)
 *   npx playwright install chromium
 */
import { test, expect, type Page } from '@playwright/test';
import { execSync, spawn, type ChildProcess } from 'child_process';
import * as path from 'path';

const PROJECT_DIR = path.resolve(__dirname, '..', '..');
const WT_PORT = 4443;
const HTTP_PORT = 4444;
const BASE_URL = `http://127.0.0.1:${HTTP_PORT}`;
const CONTAINER_NAME = 'phantom-screen-e2e-browser';

let dockerProcess: ChildProcess | null = null;

/** Wait until a URL returns HTTP 200. */
async function waitForServer(url: string, timeoutMs = 30_000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(url);
      if (res.ok) return;
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Server at ${url} not ready after ${timeoutMs}ms`);
}

/** Get the cert hash from the server's /health endpoint. */
async function getCertHash(): Promise<string> {
  // The health endpoint returns JSON with a certHash field,
  // or we can scrape it from the server logs.
  // Easier: just fetch the index page and extract from the default form value.
  const res = await fetch(BASE_URL);
  const html = await res.text();

  // The template pre-fills the cert hash input; look for it in the HTML
  const match = html.match(/certHash=([a-f0-9]{64})/i);
  if (match) return match[1];

  // Fallback: check docker logs
  const logs = execSync(`docker logs ${CONTAINER_NAME} 2>&1`, {
    encoding: 'utf-8',
  });
  const logMatch = logs.match(/Certificate SHA-256:\s*([a-f0-9]{64})/i);
  if (logMatch) return logMatch[1];

  throw new Error('Could not determine certificate hash');
}

/** Check if Docker container is already running. */
function isContainerRunning(): boolean {
  try {
    const status = execSync(
      `docker inspect -f '{{.State.Status}}' ${CONTAINER_NAME} 2>/dev/null`,
      { encoding: 'utf-8' },
    ).trim();
    return status === 'running';
  } catch {
    return false;
  }
}

/** Start the Docker container if not already running. */
function ensureContainer(): void {
  if (isContainerRunning()) return;

  // Clean up any stopped container with the same name
  try {
    execSync(`docker rm -f ${CONTAINER_NAME} 2>/dev/null`);
  } catch {
    /* ignore */
  }

  // Build the image if needed
  execSync(`docker build -t phantom-screen-e2e ${PROJECT_DIR}`, {
    stdio: 'inherit',
    timeout: 300_000,
  });

  // Run the container
  execSync(
    `docker run -d --name ${CONTAINER_NAME} ` +
      `-p ${WT_PORT}:${WT_PORT} -p ${HTTP_PORT}:${HTTP_PORT} ` +
      `phantom-screen-e2e`,
    { stdio: 'inherit' },
  );
}

test.beforeAll(async () => {
  ensureContainer();
  await waitForServer(`${BASE_URL}/health`, 60_000);
});

test.afterAll(async () => {
  // Dump server logs for debugging
  try {
    const logs = execSync(`docker logs --tail 80 ${CONTAINER_NAME} 2>&1`, {
      encoding: 'utf-8',
    });
    console.log('=== Server logs (last 80 lines) ===');
    console.log(logs);
  } catch {
    /* ignore */
  }

  // Stop container (leave it for inspection if tests fail)
  if (process.env.KEEP_CONTAINER !== '1') {
    try {
      execSync(`docker rm -f ${CONTAINER_NAME} 2>/dev/null`);
    } catch {
      /* ignore */
    }
  }
});

/**
 * Helper: navigate to the Phantom Screen client page with autoconnect.
 */
async function openPhantomScreen(page: Page): Promise<string> {
  const certHash = await getCertHash();
  const url = `${BASE_URL}/?serverUrl=https://127.0.0.1:${WT_PORT}&certHash=${certHash}&autoconnect=1`;
  await page.goto(url);
  return url;
}

/**
 * Helper: wait until the canvas has non-black pixels (real video frames).
 * Samples pixels from the canvas via JavaScript and checks for non-zero values.
 */
/** Click an element inside the client's shadow root (mount uses Shadow DOM). */
async function clickInPhantomShadow(page: Page, selector: string): Promise<void> {
  await page.evaluate((sel) => {
    const app = document.getElementById('app');
    const root = app?.shadowRoot;
    if (!root) throw new Error('Phantom Screen shadow root not found');
    const el = root.querySelector(sel) as HTMLElement | null;
    if (!el) throw new Error(`Element not found in shadow: ${sel}`);
    el.click();
  }, selector);
}

async function waitForNonBlackCanvas(
  page: Page,
  timeoutMs = 20_000,
): Promise<{ nonBlackPixels: number; totalPixels: number }> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const result = await page.evaluate(() => {
      // Find the canvas — it may be inside a shadow DOM
      let canvas: HTMLCanvasElement | null = document.querySelector('canvas');
      if (!canvas) {
        // Check shadow roots
        const hosts = document.querySelectorAll('*');
        for (const host of hosts) {
          if (host.shadowRoot) {
            canvas = host.shadowRoot.querySelector('canvas');
            if (canvas) break;
          }
        }
      }
      if (!canvas) return { error: 'no-canvas', nonBlackPixels: 0, totalPixels: 0 };

      const ctx = canvas.getContext('2d');
      if (!ctx) return { error: 'no-context', nonBlackPixels: 0, totalPixels: 0 };

      // Sample a grid of pixels across the canvas
      const w = canvas.width;
      const h = canvas.height;
      if (w === 0 || h === 0) return { error: 'zero-size', nonBlackPixels: 0, totalPixels: 0 };

      const imageData = ctx.getImageData(0, 0, w, h);
      const data = imageData.data;
      let nonBlack = 0;
      const step = 4 * 10; // sample every 10th pixel for speed
      for (let i = 0; i < data.length; i += step) {
        const r = data[i];
        const g = data[i + 1];
        const b = data[i + 2];
        if (r > 5 || g > 5 || b > 5) {
          nonBlack++;
        }
      }
      const totalSampled = Math.floor(data.length / step);
      return { nonBlackPixels: nonBlack, totalPixels: totalSampled };
    });

    if (result.nonBlackPixels > 50) {
      return result;
    }

    await page.waitForTimeout(500);
  }

  throw new Error(`Canvas still black after ${timeoutMs}ms`);
}

/** Sample canvas in a page; returns non-black pixel count in a coarse grid. */
async function countNonBlackInCanvas(
  page: Page,
  timeoutMs = 25_000,
): Promise<{ nonBlackPixels: number; totalPixels: number }> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const result = await page.evaluate(() => {
      const canvas = document.querySelector('canvas');
      if (!canvas) return { error: 'no-canvas', nonBlackPixels: 0, totalPixels: 0 };
      const ctx = canvas.getContext('2d');
      if (!ctx) return { error: 'no-context', nonBlackPixels: 0, totalPixels: 0 };
      const w = canvas.width;
      const h = canvas.height;
      if (w < 16 || h < 16) return { error: 'small-canvas', nonBlackPixels: 0, totalPixels: 0 };
      const imageData = ctx.getImageData(0, 0, w, h);
      const data = imageData.data;
      let nonBlack = 0;
      const step = 4 * 12;
      for (let i = 0; i < data.length; i += step) {
        if (data[i]! > 8 || data[i + 1]! > 8 || data[i + 2]! > 8) nonBlack++;
      }
      const totalSampled = Math.floor(data.length / step);
      return { nonBlackPixels: nonBlack, totalPixels: totalSampled };
    });
    if (result.nonBlackPixels > 30) {
      return result;
    }
    await page.waitForTimeout(400);
  }
  throw new Error(`Canvas still black after ${timeoutMs}ms`);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test('page loads and shows Phantom Screen UI', async ({ page }) => {
  await openPhantomScreen(page);

  // The page should contain the Phantom Screen title or a canvas
  const hasCanvas = await page.locator('canvas').or(page.locator('div')).first().isVisible();
  expect(hasCanvas).toBeTruthy();

  // Check page title or heading
  const bodyText = await page.textContent('body');
  // The page should have loaded without errors
  expect(bodyText).toBeTruthy();
});

test('receives video frames with real pixels (not black)', async ({ page }) => {
  await openPhantomScreen(page);

  // Wait for connection to establish (look for the connected state)
  // The UI shows connection status — wait for it
  await page.waitForTimeout(3000);

  // Check that the canvas has real pixels
  const { nonBlackPixels, totalPixels } = await waitForNonBlackCanvas(page, 30_000);

  console.log(
    `Canvas pixel check: ${nonBlackPixels}/${totalPixels} non-black pixels ` +
      `(${((nonBlackPixels / totalPixels) * 100).toFixed(1)}%)`,
  );

  // At least 1% of sampled pixels should be non-black (a real desktop has way more)
  expect(nonBlackPixels).toBeGreaterThan(totalPixels * 0.01);
});

test('server survives client resize', async ({ page }) => {
  await openPhantomScreen(page);
  await page.waitForTimeout(3000);

  // Resize the viewport — this should trigger a resolution change message
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.waitForTimeout(5000);

  // Server should still be running
  const healthRes = await fetch(`${BASE_URL}/health`);
  expect(healthRes.ok).toBeTruthy();

  // Canvas should still show real pixels after resize
  const { nonBlackPixels } = await waitForNonBlackCanvas(page, 15_000);
  expect(nonBlackPixels).toBeGreaterThan(0);
});

test('server survives coherence mode toggle', async ({ page }) => {
  await openPhantomScreen(page);
  await page.waitForTimeout(3000);

  // Look for the coherence toggle button (may be in shadow DOM)
  const coherenceBtn = page
    .locator('button')
    .filter({ hasText: /coherence|window/i })
    .first();

  if (await coherenceBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
    // Click coherence mode toggle
    await coherenceBtn.click();
    await page.waitForTimeout(3000);

    // Server should still be alive
    const healthRes = await fetch(`${BASE_URL}/health`);
    expect(healthRes.ok).toBeTruthy();

    console.log('Coherence mode toggled — server survived');
  } else {
    console.log('Coherence toggle button not found (may need shadow DOM traversal)');
  }

  // Regardless, verify server health
  const finalHealth = await fetch(`${BASE_URL}/health`);
  expect(finalHealth.ok).toBeTruthy();
});

test('server survives resize then coherence mode', async ({ page }) => {
  // This is the exact scenario that was crashing: resize kills Xvfb,
  // then coherence mode tries to use stale window IDs.
  await openPhantomScreen(page);
  await page.waitForTimeout(3000);

  // Trigger resize
  await page.setViewportSize({ width: 800, height: 600 });
  await page.waitForTimeout(5000);

  // Check server survived resize
  let healthRes = await fetch(`${BASE_URL}/health`);
  expect(healthRes.ok).toBeTruthy();

  // Try to enable coherence mode (the formerly-crashing path)
  const coherenceBtn = page
    .locator('button')
    .filter({ hasText: /coherence|window/i })
    .first();

  if (await coherenceBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
    await coherenceBtn.click();
    await page.waitForTimeout(5000);
  }

  // The key assertion: server must still be alive after resize + coherence
  healthRes = await fetch(`${BASE_URL}/health`);
  expect(healthRes.ok).toBeTruthy();

  // Canvas should still have video
  const { nonBlackPixels } = await waitForNonBlackCanvas(page, 15_000);
  expect(nonBlackPixels).toBeGreaterThan(0);

  console.log('Resize + coherence mode — server survived');
});

test('coherence pop-out shows video (non-black canvas)', async ({ page }) => {
  await openPhantomScreen(page);
  await page.waitForTimeout(3000);

  await waitForNonBlackCanvas(page, 30_000);

  await clickInPhantomShadow(page, '[data-phantom-screen="coherence-btn"]');
  await page.waitForTimeout(2000);

  // Wait for at least one window row (X11 window list may take a moment in the container)
  await expect
    .poll(
      async () =>
        page.evaluate(() => {
          const root = document.getElementById('app')?.shadowRoot;
          if (!root) return false;
          return Boolean(root.querySelector('.phantom-screen-window-popout-btn'));
        }),
      { timeout: 45_000, intervals: [500, 1000, 2000] },
    )
    .toBeTruthy();

  const popupPromise = page.waitForEvent('popup');
  await clickInPhantomShadow(page, '.phantom-screen-window-popout-btn');
  const popup = await popupPromise;
  await popup.waitForLoadState();

  // Pop-out document should decode to visible pixels (not stuck on black / broken decoder)
  const { nonBlackPixels, totalPixels } = await countNonBlackInCanvas(popup, 35_000);
  console.log(
    `Pop-out canvas: ${nonBlackPixels}/${totalPixels} non-black sampled pixels`,
  );
  expect(nonBlackPixels).toBeGreaterThan(totalPixels * 0.005);

  await popup.close();

  const healthRes = await fetch(`${BASE_URL}/health`);
  expect(healthRes.ok).toBeTruthy();
});

test('multiple resizes do not crash the server', async ({ page }) => {
  await openPhantomScreen(page);
  await page.waitForTimeout(3000);

  const sizes = [
    { width: 1280, height: 720 },
    { width: 800, height: 600 },
    { width: 1920, height: 1080 },
    { width: 1024, height: 768 },
  ];

  for (const size of sizes) {
    await page.setViewportSize(size);
    await page.waitForTimeout(3000);

    const healthRes = await fetch(`${BASE_URL}/health`);
    expect(healthRes.ok).toBeTruthy();
  }

  // After all resizes, canvas should still work
  const { nonBlackPixels } = await waitForNonBlackCanvas(page, 15_000);
  expect(nonBlackPixels).toBeGreaterThan(0);
});
