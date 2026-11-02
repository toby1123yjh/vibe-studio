import { getCurrentHostId, useHostId } from '@/shared/providers/HostIdProvider';
import {
  useQuery,
  useMutation,
  useQueryClient,
  type UseQueryResult,
} from '@tanstack/react-query';
import {
  createWorkflowApi,
  type ApproveNodeRequest,
  type RejectNodeRequest,
  type SelectArenaWinnerRequest,
  type SelectConditionBranchRequest,
} from '@/shared/lib/workflowApi';
import type { WorkflowRunResponse, TriggerWorkflowRequest } from 'shared/types';
import { shouldRefreshWorkflowRun } from '@/features/workflow/model/workflowRuntimeView';

export const workflowRunQueryKeys = {
  all: ['workflow-runs'] as const,
  host: (hostId = getCurrentHostId()) => ['workflow-runs', hostId] as const,
  detail: (runId: string, hostId = getCurrentHostId()) =>
    ['workflow-runs', hostId, 'detail', runId] as const,
};

export interface UseWorkflowRunOptions {
  enabled?: boolean;
  refetchIntervalMs?: number | false;
}

const DEFAULT_REFETCH_INTERVAL_MS = 4000;

export function useWorkflowRun(
  runId: string | null | undefined,
  options: UseWorkflowRunOptions = {}
): UseQueryResult<WorkflowRunResponse> {
  const hostId = useHostId();
  const workflowApi = createWorkflowApi(hostId);
  const { enabled = true, refetchIntervalMs = DEFAULT_REFETCH_INTERVAL_MS } =
    options;

  return useQuery({
    queryKey: runId
      ? workflowRunQueryKeys.detail(runId, hostId)
      : ['workflow-runs', 'noop'],
    queryFn: () => workflowApi.getRun(runId as string),
    enabled: !!runId && enabled,
    refetchInterval: (query) => {
      if (refetchIntervalMs === false) return false;
      const data = query.state.data as WorkflowRunResponse | undefined;
      if (!data) return false;
      return shouldRefreshWorkflowRun(data) ? refetchIntervalMs : false;
    },
    refetchOnWindowFocus: false,
  });
}

export function useWorkflowRunMutations() {
  const hostId = useHostId();
  const workflowApi = createWorkflowApi(hostId);
  const queryClient = useQueryClient();

  const triggerMutation = useMutation({
    mutationFn: ({
      workflowId,
      payload,
    }: {
      workflowId: string;
      payload: TriggerWorkflowRequest;
    }) => workflowApi.trigger(workflowId, payload),
    onSuccess: (data) => {
      queryClient.setQueryData(
        workflowRunQueryKeys.detail(data.id, hostId),
        data
      );
    },
  });

  const cancelMutation = useMutation({
    mutationFn: (runId: string) => workflowApi.cancelRun(runId),
    onSuccess: (_, runId) => {
      void queryClient.invalidateQueries({
        queryKey: workflowRunQueryKeys.detail(runId, hostId),
      });
    },
  });

  const approveMutation = useMutation({
    mutationFn: ({
      runId,
      nodeId,
      payload,
    }: {
      runId: string;
      nodeId: string;
      payload: ApproveNodeRequest;
    }) => workflowApi.approve(runId, nodeId, payload),
    onSuccess: (_, variables) => {
      void queryClient.invalidateQueries({
        queryKey: workflowRunQueryKeys.detail(variables.runId, hostId),
      });
    },
  });

  const rejectMutation = useMutation({
    mutationFn: ({
      runId,
      nodeId,
      payload,
    }: {
      runId: string;
      nodeId: string;
      payload: RejectNodeRequest;
    }) => workflowApi.reject(runId, nodeId, payload),
    onSuccess: (_, variables) => {
      void queryClient.invalidateQueries({
        queryKey: workflowRunQueryKeys.detail(variables.runId, hostId),
      });
    },
  });

  const retryMutation = useMutation({
    mutationFn: ({ runId, nodeId }: { runId: string; nodeId: string }) =>
      workflowApi.retry(runId, nodeId),
    onSuccess: (_, variables) => {
      void queryClient.invalidateQueries({
        queryKey: workflowRunQueryKeys.detail(variables.runId, hostId),
      });
    },
  });

  const selectArenaWinnerMutation = useMutation({
    mutationFn: ({
      runId,
      nodeId,
      payload,
    }: {
      runId: string;
      nodeId: string;
      payload: SelectArenaWinnerRequest;
    }) => workflowApi.selectArenaWinner(runId, nodeId, payload),
    onSuccess: (_, variables) => {
      void queryClient.invalidateQueries({
        queryKey: workflowRunQueryKeys.detail(variables.runId, hostId),
      });
    },
  });

  const selectConditionBranchMutation = useMutation({
    mutationFn: ({
      runId,
      nodeId,
      payload,
    }: {
      runId: string;
      nodeId: string;
      payload: SelectConditionBranchRequest;
    }) => workflowApi.selectConditionBranch(runId, nodeId, payload),
    onSuccess: (_, variables) => {
      void queryClient.invalidateQueries({
        queryKey: workflowRunQueryKeys.detail(variables.runId, hostId),
      });
    },
  });

  return {
    triggerRun: triggerMutation.mutateAsync,
    isTriggering: triggerMutation.isPending,
    cancelRun: cancelMutation.mutateAsync,
    isCanceling: cancelMutation.isPending,
    approveNode: approveMutation.mutateAsync,
    isApproving: approveMutation.isPending,
    rejectNode: rejectMutation.mutateAsync,
    isRejecting: rejectMutation.isPending,
    retryNode: retryMutation.mutateAsync,
    isRetrying: retryMutation.isPending,
    selectArenaWinner: selectArenaWinnerMutation.mutateAsync,
    isSelectingArenaWinner: selectArenaWinnerMutation.isPending,
    selectConditionBranch: selectConditionBranchMutation.mutateAsync,
    isSelectingConditionBranch: selectConditionBranchMutation.isPending,
  };
}
