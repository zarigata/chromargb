import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import electron from 'vite-plugin-electron/simple';

// Renderer → dist/renderer · electron main+preload → dist-electron (cjs, matches package.json "main").
// In dev the plugin launches Electron after building main — unless NO_ELECTRON=1
// (plain `vite` for browser/CI smoke tests of the renderer).
export default defineConfig({
  base: './',
  plugins: [
    react(),
    electron({
      main: {
        entry: 'electron/main.ts',
        onstart: ({ startup }) => {
          if (!process.env.NO_ELECTRON) startup();
        },
        vite: { build: { outDir: 'dist-electron' } },
      },
      preload: {
        input: 'electron/preload.ts',
        vite: { build: { outDir: 'dist-electron' } },
      },
    }),
  ],
  build: { outDir: 'dist/renderer' },
  clearScreen: false,
  server: { port: 5173, strictPort: true },
});
