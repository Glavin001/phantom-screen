import { resolve } from 'node:path';
import { defineConfig } from 'vite';

const entry = resolve(__dirname, 'src/index.ts');

export default defineConfig({
  build: {
    outDir: 'dist/html',
    emptyOutDir: false,
    target: 'es2022',
    sourcemap: true,
    lib: {
      entry,
      name: 'PhantomScreenClient',
      formats: ['iife'],
      fileName: () => 'phantom-screen-client.iife.js',
    },
  },
});
