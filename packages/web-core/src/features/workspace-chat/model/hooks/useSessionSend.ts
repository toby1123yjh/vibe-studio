import { useCallback, useState } from 'react';
import type { ExecutorConfig, SelectedSkill, Session } from 'shared/types';
import { sessionsApi } from '@/shared/lib/api';
import { useCreateSession } from './useCreateSession';

interface UseSessionSendOptions {
  /** Session ID for existing sessions */
  sessionId: string | undefined;
  /** Workspace ID for creating new sessions */
  workspaceId: string | undefined;
  /** Whether in new session mode */
  isNewSessionMode: boolean;
  /** Callback when session is selected (to exit new session mode) */
  onSelectSession?: (sessionId: string) => void;
  /** Unified executor config (executor + variant + overrides) */
  executorConfig?: ExecutorConfig | null;
}

interface UseSessionSendResult {
  /** Send a message. Returns true on success, false on failure. */
  send: (
    message: string,
    selectedSkills?: SelectedSkill[],
    options?: {
      resumeSessionId?: string | null;
      resumeScopePath?: string | null;
    }
  ) => Promise<boolean>;
  /** Whether a send operation is in progress */
  isSending: boolean;
  /** Error message if send failed */
  error: string | null;
  /** Clear the error */
  clearError: () => void;
}

/**
 * Hook for sending messages in SessionChatBoxContainer.
 * Handles both new session creation and existing session follow-up.
 *
 * Unlike useFollowUpSend, this hook:
 * - Takes message/variant as parameters to send() (not captured in closure)
 * - Returns boolean for success/failure (caller handles cleanup)
 * - Has no prompt composition (no conflict/review/clicked markdown)
 */
export function useSessionSend({
  sessionId,
  workspaceId,
  isNewSessionMode,
  onSelectSession,
  executorConfig,
}: UseSessionSendOptions): UseSessionSendResult {
  const handleSessionCreated = useCallback(
    (session: Session) => {
      onSelectSession?.(session.id);
    },
    [onSelectSession]
  );
  const { mutateAsync: createSession, isPending: isCreatingSession } =
    useCreateSession({ onSessionCreated: handleSessionCreated });
  const [isSendingFollowUp, setIsSendingFollowUp] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const send = useCallback(
    async (
      message: string,
      selectedSkills: SelectedSkill[] = [],
      options: {
        resumeSessionId?: string | null;
        resumeScopePath?: string | null;
      } = {}
    ): Promise<boolean> => {
      const trimmed = message.trim();
      if (!trimmed) return false;
      if (!executorConfig) {
        setError('No executor selected');
        return false;
      }

      setError(null);

      if (isNewSessionMode) {
        // New session flow
        if (!workspaceId) {
          setError('No workspace selected');
          return false;
        }
        try {
          await createSession({
            workspaceId,
            prompt: trimmed,
            selectedSkills,
            executorConfig,
            resumeSessionId: options.resumeSessionId,
            resumeScopePath: options.resumeScopePath,
          });
          // The durable-create callback already selects this session. Selecting
          // again after follow-up acknowledgement can interrupt later navigation.
          return true;
        } catch (e: unknown) {
          const err = e as { message?: string };
          setError(
            `Session created, but failed to start the agent: ${err.message ?? 'Unknown error'}`
          );
          return false;
        }
      } else {
        // Existing session flow
        if (!sessionId) return false;
        setIsSendingFollowUp(true);
        try {
          await sessionsApi.followUp(sessionId, {
            prompt: trimmed,
            selected_skills: selectedSkills,
            executor_config: executorConfig,
            resume_session_id: options.resumeSessionId || undefined,
            resume_scope_path: options.resumeScopePath || undefined,
          });
          return true;
        } catch (e: unknown) {
          const err = e as { message?: string };
          setError(`Failed to send: ${err.message ?? 'Unknown error'}`);
          return false;
        } finally {
          setIsSendingFollowUp(false);
        }
      }
    },
    [sessionId, workspaceId, isNewSessionMode, createSession, executorConfig]
  );

  const clearError = useCallback(() => setError(null), []);

  return {
    send,
    isSending: isSendingFollowUp || isCreatingSession,
    error,
    clearError,
  };
}
