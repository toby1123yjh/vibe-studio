import { useMutation, useQueryClient } from '@tanstack/react-query';
import { sessionsApi } from '@/shared/lib/api';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { workspaceSessionKeys } from '@/shared/hooks/workspaceSessionKeys';
import { invalidateSessionDiscovery } from '@/shared/lib/sessionDiscoveryCache';
import { sessionExecutorConfigKey } from '@/shared/hooks/sessionExecutorConfigKeys';
import type {
  Session,
  CreateFollowUpAttempt,
  ExecutorConfig,
  SelectedSkill,
} from 'shared/types';

interface CreateSessionParams {
  workspaceId: string;
  prompt: string;
  selectedSkills?: SelectedSkill[];
  executorConfig: ExecutorConfig;
  resumeSessionId?: string | null;
  resumeScopePath?: string | null;
}

interface UseCreateSessionOptions {
  /** Called once the durable session row exists, before the first follow-up. */
  onSessionCreated?: (session: Session) => void;
}

/**
 * Hook for creating a new session and sending the first message.
 * Uses TanStack Query mutation for proper cache management.
 */
export function useCreateSession(options: UseCreateSessionOptions = {}) {
  const queryClient = useQueryClient();
  const hostId = useHostId();

  return useMutation({
    mutationFn: async ({
      workspaceId,
      prompt,
      selectedSkills = [],
      executorConfig,
      resumeSessionId,
      resumeScopePath,
    }: CreateSessionParams): Promise<Session> => {
      const session = await sessionsApi.create({
        workspace_id: workspaceId,
      });

      // Publish the session as soon as the durable create succeeds. The first
      // follow-up can fail independently; that must not make a real session
      // disappear from the workspace switcher.
      const sessionQueryKey = workspaceSessionKeys.byWorkspace(
        session.workspace_id,
        hostId
      );
      await queryClient.cancelQueries({ queryKey: sessionQueryKey });
      queryClient.setQueryData<Session[]>(sessionQueryKey, (current) => {
        if (!current) return [session];
        if (current.some((item) => item.id === session.id)) return current;
        return [session, ...current];
      });
      // The durable session should be visible even when the first follow-up
      // fails. Keep the shell's cross-workspace discovery in sync at the same
      // boundary instead of waiting for the agent process to start.
      void invalidateSessionDiscovery(queryClient);
      options.onSessionCreated?.(session);

      const body: CreateFollowUpAttempt = {
        prompt,
        selected_skills: selectedSkills,
        executor_config: executorConfig,
        resume_session_id: resumeSessionId || undefined,
        resume_scope_path: resumeScopePath || undefined,
      };
      try {
        await sessionsApi.followUp(session.id, body);
      } finally {
        // The selected session can have loaded null before its initial run was
        // persisted. A failed launch may also leave a durable run or binding.
        void queryClient.invalidateQueries({
          queryKey: sessionExecutorConfigKey(hostId, session.id),
        });
      }

      return session;
    },
    onSuccess: (session) => {
      const sessionQueryKey = workspaceSessionKeys.byWorkspace(
        session.workspace_id,
        hostId
      );

      // Make the newly-created session selectable before the refetch returns.
      // This avoids a transient empty list resetting the new-session selection.
      queryClient.setQueryData<Session[]>(sessionQueryKey, (current) => {
        if (!current) return [session];
        if (current.some((item) => item.id === session.id)) return current;
        return [session, ...current];
      });

      void queryClient.invalidateQueries({ queryKey: sessionQueryKey });
    },
  });
}
