import { defineConfig } from '@playwright/test';
import base from './playwright.config';

export default defineConfig({
  ...base,
  testDir: './create-task-fixture',
  testMatch: 'create-task.spec.ts',
  outputDir: `${process.cwd()}/test-results/create-task`,
  use: { ...base.use, baseURL: 'http://127.0.0.1:4198' },
  webServer: {
    command:
      'pnpm exec vite --host 127.0.0.1 --port 4198 --strictPort --config tests/project-kanban/create-task.vite.config.ts',
    url: 'http://127.0.0.1:4198',
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
