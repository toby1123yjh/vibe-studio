import { useMutation, useQueryClient } from '@tanstack/react-query';
import { sessionsApi, workspacesApi } from '@/shared/lib/api';
import { refreshShapeFallback } from '@/shared/lib/electric/collections';
import { agentConversationApi } from '@/shared/lib/agentConversationApi';
import { getAgentConversationStore } from '@/features/agent-runtime/model/agentConversationStore';
import { useHostId } from '@/shared/providers/HostIdProvider';
import type { CreateAndStartWorkspaceRequest } from 'shared/types';
import {
  PROJECT_WORKSPACES_SHAPE,
  USER_WORKSPACES_SHAPE,
} from 'shared/remote-types';
import { workspaceSummaryKeys } from '@/shared/hooks/workspaceSummaryKeys';
import { workspaceRecordKeys } from '@/shared/hooks/useWorkspaceRecord';
import { workspaceSessionKeys } from '@/shared/hooks/workspaceSessionKeys';
import { sessionExecutorConfigKey } from '@/shared/hooks/sessionExecutorConfigKeys';
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
  const hostId = useHostId();

  const createWorkspace = useMutation({
    mutationFn: async ({ data, linkToTask }: CreateWorkspaceParams) => {
      const { workspace, agent_run: agentRun } =
        await workspacesApi.createAndStart(data);

      // The start response already owns the first Run. Preserve it before the
      // destination mounts, rather than flashing an idle composer until its
      // independent conversation request catches up.
      getAgentConversationStore(
        agentRun.state.session_id,
        hostId,
        agentConversationApi
      ).applyRunSnapshot(agentRun);
      if (workspace) {
        queryClient.setQueryData(
          workspaceRecordKeys.byId(workspace.id, hostId),
          workspace
        );
        // Keep creation feedback mounted while obtaining the actual Session
        // row. Do not manufacture a Session from the Run or a second transcript.
        // A failed warm-up remains a normal destination loading/error state;
        // it must not turn a successful workspace start into a failed creation.
        await Promise.all([
          queryClient.prefetchQuery({
            queryKey: workspaceSessionKeys.byWorkspace(workspace.id, hostId),
            queryFn: () => sessionsApi.getByWorkspace(workspace.id, hostId),
            retry: false,
          }),
          queryClient.prefetchQuery({
            queryKey: sessionExecutorConfigKey(
              hostId,
              agentRun.state.session_id
            ),
            queryFn: () =>
              sessionsApi.getExecutorConfig(agentRun.state.session_id, hostId),
            retry: false,
          }),
        ]);
      }

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
