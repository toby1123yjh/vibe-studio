import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import workflowConfig from "../workflow/vite.config";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

export default defineConfig({
  root: `${repoRoot}/tests/route-transition/fixture`,
  plugins: [react()],
  cacheDir: `${repoRoot}/node_modules/.vite/route-transition`,
  resolve: workflowConfig.resolve,
  server: { fs: { allow: [repoRoot] } },
});
