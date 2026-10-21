import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { svelteTesting } from '@testing-library/svelte/vite';
import { defineConfig } from 'vitest/config';

// SvelteKit 3 takes its configuration here (no svelte.config.js).
export default defineConfig({
  plugins: [
    sveltekit({
      // Single-page app served by the Tauri webview: no prerendering, index.html fallback.
      adapter: adapter({ pages: 'build', assets: 'build', fallback: 'index.html', strict: false })
    }),
    svelteTesting()
  ],
  clearScreen: false,
  // Tauri expects a fixed dev port (see crates/ruckcode-app/tauri.conf.json devUrl).
  server: { port: 5173, strictPort: true },
  test: {
    include: ['src/**/*.{test,spec}.ts'],
    // Pure logic runs in node; component tests opt into jsdom with a docblock.
    environment: 'node',
    setupFiles: ['./src/test-setup.ts']
  }
});
