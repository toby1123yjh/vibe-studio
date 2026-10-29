import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const containerMocks = `${repoRoot}/tests/project-kanban/fixture/src/container-mocks.tsx`;

export default defineConfig({
  root: `${repoRoot}/tests/project-kanban/fixture`,
  plugins: [react()],
  resolve: {
    alias: {
      "@/shared/actions": containerMocks,
      "@/shared/hooks/useProjectContext": containerMocks,
      "@/shared/hooks/useActions": containerMocks,
      "@/shared/hooks/useAppNavigation": containerMocks,
      "@/shared/hooks/useCurrentKanbanRouteState": containerMocks,
      "@/shared/hooks/useDeleteExecutionSession": containerMocks,
      "@/shared/components/ProjectWorkspaceDefaultContext": containerMocks,
      "@/pages/kanban/ProjectRightSidebarContainer": containerMocks,
      "@/shared/lib/executionDataApi": containerMocks,
      "@/shared/lib/remoteApi": containerMocks,
      "./TaskFloatingPanelContainer": containerMocks,
      "./ProjectBoardActions": containerMocks,
      "@": `${repoRoot}/packages/web-core/src`,
      react: `${repoRoot}/packages/web-core/node_modules/react`,
      "react-dom": `${repoRoot}/packages/web-core/node_modules/react-dom`,
      "@tanstack/react-query": `${repoRoot}/packages/web-core/node_modules/@tanstack/react-query`,
      "@tanstack/react-router": `${repoRoot}/packages/web-core/node_modules/@tanstack/react-router`,
      "lucide-react": `${repoRoot}/packages/web-core/node_modules/lucide-react`,
      shared: `${repoRoot}/shared`,
    },
  },
  server: { fs: { allow: [repoRoot] } },
});
