import { useMutation, useQueryClient } from '@tanstack/react-query';
import { workspacesApi } from '@/shared/lib/api';
import { refreshShapeFallback } from '@/shared/lib/electric/collections';
import type { CreateAndStartWorkspaceRequest } from 'shared/types';
import {
  PROJECT_WORKSPACES_SHAPE,
  USER_WORKSPACES_SHAPE,
} from 'shared/remote-types';
import { workspaceSummaryKeys } from '@/shared/hooks/workspaceSummaryKeys';
import { invalidateSessionDiscovery } from '@/shared/lib/sessionDiscoveryCache';

interface CreateWorkspaceParams {
  data: CreateAndStartWorkspaceRequest;
  linkToTask?: {
    remoteProjectId: string;
    taskId: string;
  };
}

export function useCreateWorkspace() {
  const queryClient = useQueryClient();

  const createWorkspace = useMutation({
    mutationFn: async ({ data, linkToTask }: CreateWorkspaceParams) => {
      const { workspace, agent_run: agentRun } =
        await workspacesApi.createAndStart(data);

      // Start persists the first session too. Publish it before optional Issue
      // linking, whose failure must not hide an already-created session.
      void invalidateSessionDiscovery(queryClient);

      if (linkToTask && workspace && !data.linked_task) {
        await workspacesApi.linkToTask(
          workspace.id,
          linkToTask.remoteProjectId,
          linkToTask.taskId
        );
        // The optional link updates the canonical session's Issue identity.
        void invalidateSessionDiscovery(queryClient);
      }

      return { workspace, agentRun };
    },
    onSuccess: (_result, { data, linkToTask }) => {
      // Invalidate workspace summaries so they refresh with the new workspace included
      queryClient.invalidateQueries({ queryKey: workspaceSummaryKeys.all });
      // Ensure create-mode defaults refetch the latest session/model selection.
      queryClient.invalidateQueries({ queryKey: ['workspaceCreateDefaults'] });
      queryClient.invalidateQueries({ queryKey: ['project-sessions'] });

      const projectId =
        data.linked_task?.remote_project_id ??
        linkToTask?.remoteProjectId ??
        data.project_id;
      if (projectId) {
        refreshShapeFallback(PROJECT_WORKSPACES_SHAPE, {
          project_id: projectId,
        });
      }
      refreshShapeFallback(USER_WORKSPACES_SHAPE, {});
    },
    onError: (err) => {
      console.error('Failed to create workspace:', err);
    },
  });

  return { createWorkspace };
}
