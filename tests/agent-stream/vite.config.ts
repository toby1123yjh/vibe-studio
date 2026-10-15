import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
const root = process.cwd();
export default defineConfig({
  plugins: [react()],
  root: `${root}/tests/agent-stream/fixture`,
  resolve: {
    alias: [
      {
        find: "@/shared/providers/HostIdProvider",
        replacement: `${root}/tests/agent-stream/fixture/host.tsx`,
      },
      {
        find: "@/shared/lib/localApiTransport",
        replacement: `${root}/tests/agent-stream/fixture/transport.ts`,
      },
      { find: "@", replacement: `${root}/packages/web-core/src` },
      ...["react", "react-dom"].map((find) => ({
        find,
        replacement: `${root}/packages/web-core/node_modules/${find}`,
      })),
    ],
  },
  server: { fs: { allow: [root] } },
});
