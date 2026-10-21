import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
const root = fileURLToPath(new URL('../..', import.meta.url));
export default defineConfig({
  root: `${root}/tests/agent-conversation/fixture`,
  resolve: {
    alias: [
      ...[
        '@/shared/hooks/useExecutionProcessesContext',
        '@/shared/hooks/useWorkflowManagement',
        '@/shared/dialogs/scripts/ScriptFixerDialog',
      ].map((find) => ({
        find,
        replacement: `${root}/tests/agent-conversation/fixture/context.tsx`,
      })),
      {
        find: /^\.\/DisplayConversationEntry$/,
        replacement: `${root}/tests/agent-conversation/fixture/context.tsx`,
      },
      {
        find: '@/shared/providers/HostIdProvider',
        replacement: `${root}/tests/agent-stream/fixture/host.tsx`,
      },
      {
        find: '@/shared/lib/api',
        replacement: `${root}/tests/agent-conversation/fixture/transport.ts`,
      },
      {
        find: '@/shared/lib/localApiTransport',
        replacement: `${root}/tests/agent-conversation/fixture/transport.ts`,
      },
      {
        find: '@/shared/components/WYSIWYGEditor',
        replacement: `${root}/tests/agent-conversation/fixture/markdown.tsx`,
      },
      { find: '@', replacement: `${root}/packages/web-core/src` },
      { find: 'shared', replacement: `${root}/shared` },
      ...['react', 'react-dom'].map((find) => ({
        find,
        replacement: `${root}/packages/web-core/node_modules/${find}`,
      })),
    ],
  },
  server: { fs: { allow: [root] } },
});
