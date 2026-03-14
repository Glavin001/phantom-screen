import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: '*.spec.ts',
  timeout: 60_000,
  use: {
    // Allow insecure certs for self-signed WebTransport
    ignoreHTTPSErrors: true,
  },
  projects: [
    {
      name: 'chromium',
      use: {
        browserName: 'chromium',
        // WebTransport requires secure context
        launchOptions: {
          args: [
            '--ignore-certificate-errors',
            '--allow-insecure-localhost',
            // WebTransport needs actual QUIC support
            '--enable-quic',
            '--origin-to-force-quic-on=127.0.0.1:4443',
          ],
        },
      },
    },
  ],
});
