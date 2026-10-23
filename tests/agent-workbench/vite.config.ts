import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import foundationConfig from "../ui-foundations/vite.config";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

export default defineConfig({
  root: `${repoRoot}/tests/agent-workbench/fixture`,
  resolve: {
    alias: {
      "@": `${repoRoot}/packages/web-core/src`,
      react: `${repoRoot}/packages/web-core/node_modules/react`,
      "react-dom": `${repoRoot}/packages/web-core/node_modules/react-dom`,
      shared: `${repoRoot}/shared`,
    },
  },
  css: foundationConfig.css,
  server: { fs: { allow: [repoRoot] } },
});
