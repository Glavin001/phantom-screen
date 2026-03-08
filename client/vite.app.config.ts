import { defineConfig } from 'vite';

export default defineConfig({
  root: '.',
  build: {
    outDir: 'dist/standalone',
    target: 'es2022',
  },
  server: {
    port: 3000,
  },
});
