import { defineConfig } from 'vite';
import base from './vite.config';

const fixture = `${process.cwd()}/tests/project-kanban/create-task-fixture`;

export default defineConfig({
  ...base,
  root: fixture,
  resolve: {
    alias: [
      ...[
        '@/shared/hooks/useProjectContext',
        '@/shared/hooks/useOrgContext',
        '@/shared/hooks/useActions',
        '@/shared/hooks/useAzureAttachments',
        '@/shared/hooks/useAppNavigation',
        '@/shared/hooks/useCurrentKanbanRouteState',
        '@/shared/components/WYSIWYGEditor',
        '@/shared/components/SearchableTagDropdownContainer',
        '@/shared/components/ProjectWorkspaceDefaultContext',
        '@/shared/dialogs/command-bar/CommandBarDialog',
        '@/shared/dialogs/command-bar/selections/ProjectSelectionDialog',
        '@/shared/dialogs/command-bar/LinkPrToTaskDialog',
        '@/shared/dialogs/kanban/AssigneeSelectionDialog',
        '@/shared/lib/remoteApi',
      ].map((find) => ({ find, replacement: `${fixture}/mocks.tsx` })),
      {
        find: /^\.\/Task(Comments|SubTasks|Relationships|Arena|ExecutionAttempts)SectionContainer$/,
        replacement: `${fixture}/mocks.tsx`,
      },
      ...Object.entries(base.resolve!.alias!).map(([find, replacement]) => ({
        find,
        replacement: replacement as string,
      })),
    ],
  },
});
