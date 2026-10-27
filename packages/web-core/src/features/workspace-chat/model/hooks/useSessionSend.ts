import { useCallback, useState } from 'react';
import type { ExecutorConfig, SelectedSkill, Session } from 'shared/types';
import { sessionsApi } from '@/shared/lib/api';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { useCanonicalAgentSession } from '../contexts/EntriesContext';
import { useCreateSession } from './useCreateSession';
import {
  acceptConversationSend,
  beginConversationSend,
  clearConversationSendError,
  failConversationSend,
  usePendingConversationSend,
  type ConversationSendHandle,
} from '../pendingConversationSends';

interface SessionSendOptions {
  resumeSessionId?: string | null;
  resumeScopePath?: string | null;
  /** Original visible text before contextual prompt composition. */
  displayMessage?: string;
  /** Runs synchronously only after this request owns the scoped send lock. */
  onSubmitted?: (handle: ConversationSendHandle) => void;
}

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
    options?: SessionSendOptions
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
  const hostId = useHostId();
  const { store: conversationStore } = useCanonicalAgentSession();
  const scope = {
    hostId,
    workspaceId,
    sessionId: isNewSessionMode ? undefined : sessionId,
  };
  const pendingSend = usePendingConversationSend(scope);
  const handleSessionCreated = useCallback(
    (session: Session) => {
      onSelectSession?.(session.id);
    },
    [onSelectSession]
  );
  const { mutateAsync: createSession } = useCreateSession({
    onSessionCreated: handleSessionCreated,
  });
  const [error, setError] = useState<string | null>(null);

  const send = useCallback(
    async (
      message: string,
      selectedSkills: SelectedSkill[] = [],
      options: SessionSendOptions = {}
    ): Promise<boolean> => {
      const trimmed = message.trim();
      if (!trimmed) return false;
      if (!executorConfig) {
        setError('No executor selected');
        return false;
      }
      if (!workspaceId || (!isNewSessionMode && !sessionId)) return false;
      const handle = beginConversationSend({
        hostId,
        workspaceId,
        sessionId: isNewSessionMode ? undefined : sessionId,
        text: options.displayMessage?.trim() ? options.displayMessage : trimmed,
        executorConfig,
        baselineRunIds: conversationStore
          ?.getSnapshot()
          .timeline?.runs.map((run) => run.summary.agent_run_id),
      });
      if (!handle) return false;

      setError(null);
      options.onSubmitted?.(handle);

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
            pendingSend: handle,
          });
          // The durable-create callback already selects this session. Selecting
          // again after follow-up acknowledgement can interrupt later navigation.
          return true;
        } catch (e: unknown) {
          const err = e as { message?: string };
          failConversationSend(
            handle,
            `Session created, but failed to start the agent: ${err.message ?? 'Unknown error'}`
          );
          return false;
        }
      } else {
        // Existing session flow
        if (!sessionId) return false;
        try {
          const run = await sessionsApi.followUp(
            sessionId,
            {
              prompt: trimmed,
              selected_skills: selectedSkills,
              executor_config: executorConfig,
              resume_session_id: options.resumeSessionId || undefined,
              resume_scope_path: options.resumeScopePath || undefined,
            },
            hostId
          );
          acceptConversationSend(handle, run.agent_run_id);
          return true;
        } catch (e: unknown) {
          const err = e as { message?: string };
          failConversationSend(
            handle,
            `Failed to send: ${err.message ?? 'Unknown error'}`
          );
          return false;
        }
      }
    },
    [
      hostId,
      sessionId,
      workspaceId,
      isNewSessionMode,
      createSession,
      executorConfig,
      conversationStore,
    ]
  );

  const clearError = useCallback(() => {
    setError(null);
    clearConversationSendError({
      hostId,
      workspaceId,
      sessionId: isNewSessionMode ? undefined : sessionId,
    });
  }, [hostId, workspaceId, sessionId, isNewSessionMode]);

  return {
    send,
    isSending: pendingSend.sends.some((send) => send.phase === 'sending'),
    error: pendingSend.error ?? error,
    clearError,
  };
}
