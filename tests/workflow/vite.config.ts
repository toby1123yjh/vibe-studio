import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

export default defineConfig({
  root: `${repoRoot}/tests/workflow/fixture`,
  // Other browser fixtures run separate Vite servers from the same repository.
  cacheDir: `${repoRoot}/node_modules/.vite/workflow`,
  plugins: [react()],
  resolve: {
    alias: {
      "@": `${repoRoot}/packages/web-core/src`,
      "@tanstack/react-query": `${repoRoot}/packages/web-core/node_modules/@tanstack/react-query`,
      "@tanstack/react-router": `${repoRoot}/packages/web-core/node_modules/@tanstack/react-router`,
      "@xyflow/react": `${repoRoot}/packages/web-core/node_modules/@xyflow/react`,
      "lucide-react": `${repoRoot}/packages/web-core/node_modules/lucide-react`,
      "react-hotkeys-hook": `${repoRoot}/packages/web-core/node_modules/react-hotkeys-hook`,
      "@ebay/nice-modal-react": `${repoRoot}/packages/web-core/node_modules/@ebay/nice-modal-react`,
      "react-i18next": `${repoRoot}/packages/web-core/node_modules/react-i18next`,
      react: `${repoRoot}/packages/web-core/node_modules/react`,
      "react-dom": `${repoRoot}/packages/web-core/node_modules/react-dom`,
      shared: `${repoRoot}/shared`,
      "shared/types": `${repoRoot}/shared/types.ts`,
      "shared/remote-types": `${repoRoot}/shared/remote-types.ts`,
    },
  },
  server: {
    fs: {
      allow: [repoRoot],
    },
  },
});
