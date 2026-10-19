import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import workflowConfig from '../workflow/vite.config';

const repoRoot = fileURLToPath(new URL('../..', import.meta.url));

export default defineConfig({
  root: `${repoRoot}/tests/app-shell/fixture`,
  cacheDir: `${repoRoot}/node_modules/.vite/app-shell`,
  plugins: [react()],
  resolve: workflowConfig.resolve,
  server: {
    fs: {
      allow: [repoRoot],
    },
  },
});
