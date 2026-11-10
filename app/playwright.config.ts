import { defineConfig, devices } from '@playwright/test';

// 4173 is vite preview's default; set E2E_PORT when something else already uses it.
const PORT = Number(process.env.E2E_PORT ?? 4173);

// Runs against the production build served by `vite preview`, with the scripted mock agent
// (`?mock=1`). Browsers come from PLAYWRIGHT_BROWSERS_PATH / `playwright install chromium`.
export default defineConfig({
  testDir: './e2e',
  timeout: 30_000,
  expect: { timeout: 8_000 },
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: 'retain-on-failure'
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: `pnpm build && pnpm preview --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: !process.env.CI,
    timeout: 180_000
  }
});
