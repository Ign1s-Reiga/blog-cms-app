import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    // Threads rather than the default forks pool. Spawning a child process per
    // test file times out on Windows here before the worker ever answers;
    // threads share the process and start immediately.
    pool: 'threads',
    setupFiles: ['./vitest.setup.mts'],
    // Only our own tests. Without this, `node_modules` is walked and every
    // dependency shipping a `.test.js` is collected.
    include: ['src/**/*.test.{ts,tsx}'],
  },
  resolve: {
    // The same `@/*` → `src/app/*` mapping `tsconfig.json` declares. Vitest
    // does not read tsconfig paths, so a mismatch here is a mismatch nobody
    // notices until an import resolves in the editor and not in the test.
    alias: {
      '@': fileURLToPath(new URL('./src/app', import.meta.url)),
    },
  },
});
