import { useCallback } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { queueApi } from '@/shared/lib/api';
import { useHostId } from '@/shared/providers/HostIdProvider';
import type { ExecutorConfig, QueueStatus, SelectedSkill } from 'shared/types';

interface UseSessionQueueInteractionOptions {
  /** Session ID for queue operations */
  sessionId: string | undefined;
}

interface UseSessionQueueInteractionResult {
  /** Whether a message is currently queued */
  isQueued: boolean;
  /** The queued message content, if any */
  queuedMessage: string | null;
  /** The executor config from the queued message, if any */
  queuedConfig: ExecutorConfig | null;
  /** Whether a queue operation is in progress */
  isQueueLoading: boolean;
  /** Queue a message for later execution */
  queueMessage: (
    message: string,
    executorConfig: ExecutorConfig,
    selectedSkills?: SelectedSkill[]
  ) => Promise<void>;
  /** Cancel the queued message */
  cancelQueue: () => Promise<void>;
  /** Refresh queue status from server */
  refreshQueueStatus: () => Promise<void>;
}

const QUEUE_STATUS_KEY = 'queue-status';

/**
 * Hook to manage queue interaction for session messages.
 * Uses TanStack Query for caching and mutation handling.
 */
export function useSessionQueueInteraction({
  sessionId,
}: UseSessionQueueInteractionOptions): UseSessionQueueInteractionResult {
  const queryClient = useQueryClient();
  const hostId = useHostId();

  // Query for queue status
  const { data: queueStatus = { status: 'empty' as const }, refetch } =
    useQuery<QueueStatus>({
      queryKey: [QUEUE_STATUS_KEY, hostId, sessionId],
      queryFn: () => queueApi.getStatus(sessionId!, hostId),
      enabled: !!sessionId,
    });

  const isQueued = queueStatus.status === 'queued';
  const queuedMessageData = isQueued
    ? (queueStatus as Extract<QueueStatus, { status: 'queued' }>).message
    : null;
  const queuedMessage = queuedMessageData?.data.message ?? null;
  const queuedConfig: ExecutorConfig | null =
    queuedMessageData?.data.executor_config ?? null;

  // Mutation for queueing a message
  const queueMutation = useMutation({
    mutationFn: ({
      message,
      executorConfig,
      selectedSkills,
      sessionId: queuedSessionId,
      hostId: queuedHostId,
    }: {
      message: string;
      executorConfig: ExecutorConfig;
      selectedSkills?: SelectedSkill[];
      sessionId: string;
      hostId: string | null;
    }) =>
      queueApi.queue(
        queuedSessionId,
        {
          message,
          executor_config: executorConfig,
          selected_skills: selectedSkills,
        },
        queuedHostId
      ),
    onSuccess: (status, variables) => {
      queryClient.setQueryData(
        [QUEUE_STATUS_KEY, variables.hostId, variables.sessionId],
        status
      );
    },
  });

  // Mutation for cancelling the queue
  const cancelMutation = useMutation({
    mutationFn: ({
      sessionId: queuedSessionId,
      hostId: queuedHostId,
    }: {
      sessionId: string;
      hostId: string | null;
    }) => queueApi.cancel(queuedSessionId, queuedHostId),
    onSuccess: (status, variables) => {
      queryClient.setQueryData(
        [QUEUE_STATUS_KEY, variables.hostId, variables.sessionId],
        status
      );
    },
  });

  const queueMessage = useCallback(
    async (
      message: string,
      executorConfig: ExecutorConfig,
      selectedSkills?: SelectedSkill[]
    ) => {
      if (!sessionId) return;
      await queueMutation.mutateAsync({
        message,
        executorConfig,
        selectedSkills,
        sessionId,
        hostId,
      });
    },
    [sessionId, hostId, queueMutation]
  );

  const cancelQueue = useCallback(async () => {
    if (!sessionId) return;
    await cancelMutation.mutateAsync({ sessionId, hostId });
  }, [sessionId, hostId, cancelMutation]);

  const refreshQueueStatus = useCallback(async () => {
    if (!sessionId) return;
    await refetch();
  }, [sessionId, refetch]);

  return {
    isQueued,
    queuedMessage,
    queuedConfig,
    isQueueLoading: queueMutation.isPending || cancelMutation.isPending,
    queueMessage,
    cancelQueue,
    refreshQueueStatus,
  };
}
