import { defineConfig, devices } from '@playwright/test';

const port = 4186;
export default defineConfig({
  testDir: './specs',
  outputDir: `${process.cwd()}/test-results/session-dashboard`,
  timeout: 35_000,
  expect: { timeout: 8_000 },
  workers: 1,
  reporter: 'list',
  use: { baseURL: `http://127.0.0.1:${port}`, screenshot: 'only-on-failure' },
  webServer: {
    command: `pnpm exec vite --host 127.0.0.1 --port ${port} --strictPort --config tests/session-dashboard/vite.config.ts`,
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'], ...(process.env.CI ? {} : { channel: 'chrome' }) } }],
});
