import { resolve } from 'node:path';
import { defineConfig } from 'vite';

const entry = resolve(__dirname, 'src/index.ts');

export default defineConfig({
  build: {
    outDir: 'dist/npm',
    emptyOutDir: false,
    target: 'es2022',
    sourcemap: true,
    lib: {
      entry,
      name: 'PhantomScreenClient',
      formats: ['es', 'cjs'],
      fileName: (format) => (format === 'es' ? 'index.js' : 'index.cjs'),
    },
    rollupOptions: {
      output: {
        exports: 'named',
      },
    },
  },
});
