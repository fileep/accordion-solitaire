import { defineConfig } from 'vite';

export default defineConfig({
  // The solver worker loads the WASM engine with import.meta.url, which needs ES-module workers.
  worker: { format: 'es' },
});
