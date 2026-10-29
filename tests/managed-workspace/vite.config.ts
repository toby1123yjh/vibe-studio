import { defineConfig } from 'vite';

const root = process.cwd();
const fixture = `${root}/tests/managed-workspace/fixture`;
export default defineConfig({
  root: fixture,
  optimizeDeps: { entries: ['index.html'] },
  resolve: {
    alias: [
      ...[
        /^\.\/SettingsHostContext$/,
        /^\.\/SettingsMachineUserSystemProvider$/,
        '@/shared/dialogs/shared/FolderPickerDialog',
      ].map((find) => ({ find, replacement: `${fixture}/settings-mocks.ts` })),
      ...[
        '@/features/create-mode/model/useCreateMode',
        '@/shared/hooks/useUserSystem',
        '@/shared/hooks/useCreateWorkspace',
        '@/shared/hooks/useAppShellProjects',
        '@/shared/lib/executionDataApi',
        '@/shared/lib/api',
        '@/shared/hooks/useCreateAttachments',
        '@/shared/hooks/useScratch',
        '@/shared/hooks/useWorkspaceCreateDefaults',
        '@/shared/integrations/electric/hooks',
        '@/shared/hooks/useExecutorConfig',
        '@/shared/hooks/useProjectRepoDefaults',
        '@/shared/hooks/useCurrentAppDestination',
        '@/shared/hooks/useSettingsNavigation',
        '@/shared/dialogs/shared/WorkspaceTargetDialog',
        '@/shared/components/AgentIcon',
        '@/shared/components/WYSIWYGEditor',
        '@/shared/components/ModelSelectorContainer',
        '@/shared/components/AgentSessionResumePicker',
        '@vibe/ui/components/CreateChatBox',
        'react-i18next',
        'react-dropzone',
      ].map((find) => ({ find, replacement: `${fixture}/mocks.tsx` })),
      { find: '@', replacement: `${root}/packages/web-core/src` },
      { find: 'shared', replacement: `${root}/shared` },
      { find: '@vibe/ui', replacement: `${root}/packages/ui/src` },
      ...['react', 'react-dom', '@tanstack/react-query'].map((find) => ({
        find,
        replacement: `${root}/packages/web-core/node_modules/${find}`,
      })),
    ],
  },
  server: { fs: { allow: [root] } },
});
