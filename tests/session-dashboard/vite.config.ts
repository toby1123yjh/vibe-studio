import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import workflowConfig from '../workflow/vite.config';

const repoRoot = fileURLToPath(new URL('../..', import.meta.url));

export default defineConfig({
  ...workflowConfig,
  root: `${repoRoot}/tests/session-dashboard/fixture`,
  cacheDir: `${repoRoot}/node_modules/.vite/session-dashboard`,
  css: { postcss: `${repoRoot}/tests/session-dashboard` },
});
