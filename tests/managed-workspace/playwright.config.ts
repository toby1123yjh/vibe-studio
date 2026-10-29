import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './specs',
  outputDir: `${process.cwd()}/test-results/managed-workspace`,
  testIgnore: 'general.spec.ts',
  workers: 1,
  reporter: 'list',
  use: { baseURL: 'http://127.0.0.1:4204', channel: 'chrome' },
  webServer: {
    command:
      'pnpm exec vite --host 127.0.0.1 --port 4204 --strictPort --config tests/managed-workspace/vite.config.ts',
    url: 'http://127.0.0.1:4204',
    reuseExistingServer: false,
  },
});
