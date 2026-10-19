import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '../../packages/web-core/src/shared/lib',
  testMatch: ['modelSelector.test.ts'],
  workers: 1,
  reporter: 'list',
});
