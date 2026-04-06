import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  timeout: 120_000,
  retries: 0,
  reporter: 'list',
  use: {
    // WebTransport requires a real Chromium with QUIC support.
    // The self-signed cert is handled via serverCertificateHashes in the
    // WebTransport API, but Chromium also needs to allow insecure localhost.
    browserName: 'chromium',
    launchOptions: {
      args: [
        '--origin-to-force-quic-on=127.0.0.1:4443',
        '--ignore-certificate-errors',
        '--enable-quic',
        // Coherence "Pop Out" uses window.open; avoid flaky blocked-popup behavior in CI.
        '--disable-popup-blocking',
      ],
    },
    headless: true,
    video: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
});
