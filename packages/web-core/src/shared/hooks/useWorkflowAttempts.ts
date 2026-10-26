import { getCurrentHostId, useHostId } from '@/shared/providers/HostIdProvider';
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
  queryOptions,
  type QueryClient,
  type UseQueryResult,
} from '@tanstack/react-query';
import {
  createWorkflowApi,
  type CreateWorkflowAttemptPayload,
  type RunWorkflowAttemptPayload,
} from '@/shared/lib/workflowApi';
import type {
  WorkflowAttemptListResponse,
  WorkflowAttemptResponse,
  WorkflowAttemptStatus,
  WorkflowRunStatus,
} from 'shared/types';
import { workflowRunQueryKeys } from './useWorkflowRun';
import { workflowTemplateQueryKeys } from './useWorkflowTemplates';

export const workflowAttemptQueryKeys = {
  all: ['workflow-attempts'] as const,
  host: (hostId = getCurrentHostId()) => ['workflow-attempts', hostId] as const,
  project: (projectId: string, hostId = getCurrentHostId()) =>
    ['workflow-attempts', hostId, 'project', projectId] as const,
  task: (projectId: string, taskId: string, hostId = getCurrentHostId()) =>
    [
      'workflow-attempts',
      hostId,
      'project',
      projectId,
      'task',
      taskId,
    ] as const,
  detail: (attemptId: string, hostId = getCurrentHostId()) =>
    ['workflow-attempts', hostId, 'detail', attemptId] as const,
  workflow: (workflowId: string, hostId = getCurrentHostId()) =>
    ['workflow-attempts', hostId, 'workflow', workflowId] as const,
};

function workflowAttemptStatusFromRunStatus(
  status: WorkflowRunStatus
): WorkflowAttemptStatus {
  return status === 'pending' ? 'ready' : status;
}

function updateCachedWorkflowAttempt(
  queryClient: QueryClient,
  attemptId: string,
  updater: (attempt: WorkflowAttemptResponse) => WorkflowAttemptResponse | null,
  hostId: string | null
) {
  queryClient
    .getQueriesData<WorkflowAttemptListResponse>({
      queryKey: workflowAttemptQueryKeys.host(hostId),
    })
    .forEach(([queryKey, data]) => {
      if (!data?.attempts) return;
      queryClient.setQueryData<WorkflowAttemptListResponse>(queryKey, {
        ...data,
        attempts: data.attempts
          .map((attempt) =>
            attempt.id === attemptId ? updater(attempt) : attempt
          )
          .filter((attempt): attempt is WorkflowAttemptResponse =>
            Boolean(attempt)
          ),
      });
    });

  queryClient
    .getQueriesData<WorkflowAttemptResponse | null>({
      queryKey: workflowAttemptQueryKeys.host(hostId),
    })
    .forEach(([queryKey, data]) => {
      if (data?.id === attemptId) {
        queryClient.setQueryData(queryKey, updater(data));
      }
    });
}

export function useWorkflowAttempts(
  projectId: string | null | undefined,
  taskId: string | null | undefined,
  options: { enabled?: boolean } = {}
): UseQueryResult<WorkflowAttemptListResponse> {
  const hostId = useHostId();
  const workflowApi = createWorkflowApi(hostId);
  const { enabled = true } = options;

  return useQuery({
    queryKey:
      projectId && taskId
        ? workflowAttemptQueryKeys.task(projectId, taskId, hostId)
        : ['workflow-attempts', 'noop'],
    queryFn: () =>
      workflowApi.listAttempts(projectId as string, taskId as string),
    enabled: !!projectId && !!taskId && enabled,
  });
}

export function useProjectWorkflowAttempts(
  projectId: string | null | undefined,
  options: { enabled?: boolean } = {}
): UseQueryResult<WorkflowAttemptListResponse> {
  const hostId = useHostId();
  const { enabled = true } = options;

  return useQuery({
    ...projectWorkflowAttemptsQueryOptions(projectId ?? '', hostId),
    enabled: !!projectId && enabled,
  });
}

function projectWorkflowAttemptsQueryOptions(
  projectId: string,
  hostId: string | null
) {
  return queryOptions({
    queryKey: workflowAttemptQueryKeys.project(projectId, hostId),
    queryFn: ({ signal }) =>
      createWorkflowApi(hostId).listProjectAttempts(projectId, signal),
    refetchInterval: (query) =>
      query.state.data?.attempts.some((attempt) =>
        ['running', 'awaiting_human', 'awaiting_arena', 'cancelling'].includes(
          attempt.status
        )
      )
        ? 5_000
        : false,
  });
}

export function useWorkflowAttemptsForProjects(projectIds: readonly string[]) {
  const hostId = useHostId();
  return useQueries({
    queries: projectIds.map((projectId) =>
      projectWorkflowAttemptsQueryOptions(projectId, hostId)
    ),
  });
}

export function useWorkflowAttemptForWorkflow(
  workflowId: string | null | undefined,
  options: { enabled?: boolean } = {}
): UseQueryResult<WorkflowAttemptResponse | null> {
  const hostId = useHostId();
  const workflowApi = createWorkflowApi(hostId);
  const { enabled = true } = options;

  return useQuery({
    queryKey: workflowId
      ? workflowAttemptQueryKeys.workflow(workflowId, hostId)
      : ['workflow-attempts', 'workflow', 'noop'],
    queryFn: () => workflowApi.getAttemptForWorkflow(workflowId as string),
    enabled: !!workflowId && enabled,
    refetchInterval: 5_000,
  });
}

export function useWorkflowAttemptMutations() {
  const hostId = useHostId();
  const workflowApi = createWorkflowApi(hostId);
  const queryClient = useQueryClient();

  const createAttemptMutation = useMutation({
    mutationFn: ({
      projectId,
      taskId,
      payload,
    }: {
      projectId: string;
      taskId: string;
      payload: CreateWorkflowAttemptPayload;
    }) => workflowApi.createAttempt(projectId, taskId, payload),
    onSuccess: (attempt, variables) => {
      void queryClient.invalidateQueries({
        queryKey: workflowAttemptQueryKeys.task(
          variables.projectId,
          variables.taskId,
          hostId
        ),
      });
      void queryClient.invalidateQueries({
        queryKey: workflowAttemptQueryKeys.project(variables.projectId, hostId),
      });
      void queryClient.invalidateQueries({
        queryKey: workflowTemplateQueryKeys.list(variables.projectId, hostId),
      });
      queryClient.setQueryData(
        workflowAttemptQueryKeys.detail(attempt.id, hostId),
        attempt
      );
      queryClient.setQueryData(
        workflowAttemptQueryKeys.workflow(attempt.workflow_id, hostId),
        attempt
      );
    },
  });

  const runAttemptMutation = useMutation({
    mutationFn: ({
      attemptId,
      payload,
    }: {
      attemptId: string;
      payload: RunWorkflowAttemptPayload;
    }) => workflowApi.runAttempt(attemptId, payload),
    onSuccess: (run) => {
      queryClient.setQueryData(
        workflowRunQueryKeys.detail(run.id, hostId),
        run
      );
      if (run.attempt_id) {
        updateCachedWorkflowAttempt(
          queryClient,
          run.attempt_id,
          (attempt) => ({
            ...attempt,
            latest_run_id: run.id,
            workspace_id: run.workspace_id ?? attempt.workspace_id,
            status: workflowAttemptStatusFromRunStatus(run.status),
            updated_at: run.updated_at,
          }),
          hostId
        );
      }
      void queryClient.invalidateQueries({
        queryKey: workflowAttemptQueryKeys.host(hostId),
      });
      if (run.workflow_id) {
        void queryClient.invalidateQueries({
          queryKey: workflowAttemptQueryKeys.workflow(run.workflow_id, hostId),
        });
      }
    },
  });

  const deleteAttemptMutation = useMutation({
    mutationFn: (attemptId: string) => workflowApi.deleteAttempt(attemptId),
    onMutate: async (attemptId) => {
      await queryClient.cancelQueries({
        queryKey: workflowAttemptQueryKeys.host(hostId),
      });

      const previousLists =
        queryClient.getQueriesData<WorkflowAttemptListResponse>({
          queryKey: workflowAttemptQueryKeys.host(hostId),
        });
      const previousDetails =
        queryClient.getQueriesData<WorkflowAttemptResponse | null>({
          queryKey: workflowAttemptQueryKeys.host(hostId),
        });

      previousLists.forEach(([queryKey, data]) => {
        if (!data?.attempts) return;
        queryClient.setQueryData<WorkflowAttemptListResponse>(queryKey, {
          ...data,
          attempts: data.attempts.filter((attempt) => attempt.id !== attemptId),
        });
      });

      previousDetails.forEach(([queryKey, data]) => {
        if (data?.id === attemptId) {
          queryClient.setQueryData(queryKey, null);
        }
      });

      return { previousLists, previousDetails };
    },
    onError: (_error, _attemptId, context) => {
      context?.previousLists.forEach(([queryKey, data]) => {
        queryClient.setQueryData(queryKey, data);
      });
      context?.previousDetails.forEach(([queryKey, data]) => {
        queryClient.setQueryData(queryKey, data);
      });
    },
    onSettled: () => {
      void queryClient.invalidateQueries({
        queryKey: workflowAttemptQueryKeys.host(hostId),
      });
      void queryClient.invalidateQueries({
        queryKey: workflowTemplateQueryKeys.host(hostId),
      });
    },
  });

  return {
    createAttempt: createAttemptMutation.mutateAsync,
    isCreatingAttempt: createAttemptMutation.isPending,
    runAttempt: runAttemptMutation.mutateAsync,
    isRunningAttempt: runAttemptMutation.isPending,
    deleteAttempt: deleteAttemptMutation.mutateAsync,
    isDeletingAttempt: deleteAttemptMutation.isPending,
  };
}
