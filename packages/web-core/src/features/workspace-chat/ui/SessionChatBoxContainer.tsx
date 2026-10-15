import {
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useLocation } from '@tanstack/react-router';
import { useDropzone } from 'react-dropzone';
import { useTranslation } from 'react-i18next';
import {
  BaseAgentCapability,
  type Session,
  type BaseCodingAgent,
  type ExecutorConfig,
  type SelectedSkill,
  type ResumableAgentSession,
  type Workspace,
} from 'shared/types';
import { AgentIcon } from '@/shared/components/AgentIcon';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { workspaceSessionKeys } from '@/shared/hooks/workspaceSessionKeys';
import { useWorkspaceExecution } from '@/shared/hooks/useWorkspaceExecution';
import { useWorkspaceRepo } from '@/shared/hooks/useWorkspaceRepo';
import { useWorkspaceRecord } from '@/shared/hooks/useWorkspaceRecord';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import WYSIWYGEditor from '@/shared/components/WYSIWYGEditor';
import { useApprovalFeedbackOptional } from '../model/contexts/ApprovalFeedbackContext';
import { useMessageEditContext } from '../model/contexts/MessageEditContext';
import {
  useCanonicalAgentSession,
  useEntries,
  useTokenUsage,
} from '../model/contexts/EntriesContext';
import { useReviewOptional } from '@/shared/hooks/useReview';
import { ActionsContext } from '@/shared/hooks/useActions';
import { useTodos } from '../model/hooks/useTodos';
import { useSessionExecutorConfig } from '../model/hooks/useSessionExecutorConfig';
import { useSessionMessageEditor } from '../model/hooks/useSessionMessageEditor';
import { useSessionQueueInteraction } from '../model/hooks/useSessionQueueInteraction';
import { useSessionSend } from '../model/hooks/useSessionSend';
import { useSessionAttachments } from '../model/hooks/useSessionAttachments';
import { useMessageEditRetry } from '../model/hooks/useMessageEditRetry';
import { useAgentProviderPolicy } from '@/shared/hooks/useAgentProviderPolicy';
import { useBranchStatus } from '@/shared/hooks/useBranchStatus';
import { useApprovalMutation } from '../model/hooks/useApprovalMutation';
import { useWorkspacePendingApproval } from '../model/hooks/useWorkspacePendingApproval';
import { useIsMobile } from '@/shared/hooks/useIsMobile';
import { ResolveConflictsDialog } from '@/shared/dialogs/tasks/ResolveConflictsDialog';
import { workspaceSummaryKeys } from '@/shared/hooks/workspaceSummaryKeys';
import { buildAgentPrompt } from '@/shared/lib/promptMessage';
import { formatDateShortWithTime } from '@/shared/lib/date';
import { toPrettyCase } from '@/shared/lib/string';
import {
  SessionChatBox,
  type ExecutionStatus,
  type SessionChatBoxEditorRenderProps,
} from '@vibe/ui/components/SessionChatBox';
import { ModelSelectorContainer } from '@/shared/components/ModelSelectorContainer';
import {
  AgentSessionResumeChip,
  AgentSessionResumePicker,
} from '@/shared/components/AgentSessionResumePicker';
import {
  useWorkspacePanelState,
  RIGHT_MAIN_PANEL_MODES,
} from '@/shared/stores/useUiPreferencesStore';
import { useInspectModeStore } from '../model/store/useInspectModeStore';
import { Actions } from '@/shared/actions';
import { useSettingsNavigation } from '@/shared/hooks/useSettingsNavigation';
import { PrCommentsDialog } from '@/shared/dialogs/tasks/PrCommentsDialog';
import type { NormalizedComment } from '@vibe/ui/components/pr-comment-node';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { sessionsApi } from '@/shared/lib/api';
import { useDeleteExecutionSession } from '@/shared/hooks/useDeleteExecutionSession';
import { RenameSessionDialog } from '@vibe/ui/components/RenameSessionDialog';
import type { TurnNavigationItem } from '@vibe/ui/components/TurnNavigationPopup';
import { deriveRuntimeActionPolicy } from '@/shared/lib/runtimeActionPolicy';
import { resolveWorkspaceWorkingDirectory } from '@/shared/lib/workspaceContext';
import { isExecutionProcessActive } from '@/shared/lib/executionProcessRuntime';
import { deriveCanonicalAgentRunActionPolicy } from '@/features/agent-runtime';
import { canonicalAgentControls } from '../model/canonicalAgentControls';
import {
  isSessionDraftSubmissionCurrent,
  snapshotSessionDraft,
} from '@/features/agent-workbench/model/sessionDraft';

const EMPTY_DIFF_PATHS = new Set<string>();

/** Compute execution status from boolean flags */
function computeExecutionStatus(params: {
  isInFeedbackMode: boolean;
  isInEditMode: boolean;
  isStopping: boolean;
  isQueueLoading: boolean;
  isSendingFollowUp: boolean;
  isQueued: boolean;
  isAgentRunActive: boolean;
}): ExecutionStatus {
  if (params.isInFeedbackMode) return 'feedback';
  if (params.isInEditMode) return 'edit';
  if (params.isStopping) return 'stopping';
  if (params.isQueueLoading) return 'queue-loading';
  if (params.isSendingFollowUp) return 'sending';
  if (params.isQueued) return 'queued';
  if (params.isAgentRunActive) return 'running';
  return 'idle';
}

/** Shared props across all modes */
interface SharedProps {
  /** Available sessions for this workspace */
  sessions: Session[];
  /** Number of files changed in current session */
  filesChanged: number;
  /** Number of lines added */
  linesAdded: number;
  /** Number of lines removed */
  linesRemoved: number;
  diffStatsUnavailable?: boolean;
  /** Callback to scroll to previous user message */
  onScrollToPreviousMessage: () => void;
  /** Callback to scroll to bottom of conversation */
  onScrollToBottom: (behavior?: 'auto' | 'smooth') => void;
  /** Callback to scroll to a specific user message by patchKey */
  onScrollToUserMessage: (patchKey: string) => void;
  /** Returns the patchKey of the user message currently visible in the viewport */
  getActiveTurnPatchKey?: () => string | null;
  /** Disable the "view code" click handler (for VS Code extension) */
  disableViewCode: boolean;
  /** Replace diff stats with an "Open Workspace" button in header */
  showOpenWorkspaceButton: boolean;
  /** Default executor config supplied by an embedding workflow/node context. */
  preferredExecutorConfig?: ExecutorConfig | null;
  presentation?: 'full-page' | 'embedded';
  workspace?: Workspace | null;
  diffPaths?: Set<string>;
}

/** Props for existing session mode */
interface ExistingSessionProps extends SharedProps {
  mode: 'existing-session';
  /** The current session */
  session: Session;
  /** Called when a session is selected */
  onSelectSession: (sessionId: string) => void;
  /** Callback to start new session mode */
  onStartNewSession: (() => void) | undefined;
}

/** Props for new session mode */
interface NewSessionProps extends SharedProps {
  mode: 'new-session';
  /** Workspace ID for creating new sessions */
  workspaceId: string;
  /** Called when a session is selected */
  onSelectSession: (sessionId: string) => void;
}

/** Props for placeholder mode (no workspace selected) */
interface PlaceholderProps extends SharedProps {
  mode: 'placeholder';
}

type SessionChatBoxContainerProps =
  | ExistingSessionProps
  | NewSessionProps
  | PlaceholderProps;

export function SessionChatBoxContainer(props: SessionChatBoxContainerProps) {
  const { t } = useTranslation('tasks');
  const { openAgentCenter } = useSettingsNavigation();
  const {
    mode,
    sessions,
    filesChanged,
    linesAdded,
    linesRemoved,
    diffStatsUnavailable,
    onScrollToPreviousMessage,
    onScrollToBottom,
    onScrollToUserMessage,
    getActiveTurnPatchKey,
    disableViewCode = false,
    showOpenWorkspaceButton,
    preferredExecutorConfig,
    presentation = 'full-page',
    workspace: explicitWorkspace,
    diffPaths,
  } = props;

  // Extract mode-specific values
  const session = mode === 'existing-session' ? props.session : undefined;
  const workspaceId =
    mode === 'existing-session'
      ? props.session.workspace_id
      : mode === 'new-session'
        ? props.workspaceId
        : undefined;
  const isNewSessionMode = mode === 'new-session';
  const onSelectSession =
    mode === 'placeholder' ? undefined : props.onSelectSession;
  const onStartNewSession =
    mode === 'existing-session' ? props.onStartNewSession : undefined;

  const sessionId = session?.id;
  const queryClient = useQueryClient();
  const hostId = useHostId();
  const location = useLocation();

  const handleRenameSession = useCallback(
    (targetSessionId: string, currentName: string) => {
      void RenameSessionDialog.show({
        currentName,
        onRename: async (newName: string) => {
          await sessionsApi.update(targetSessionId, { name: newName });
          void queryClient.invalidateQueries({
            queryKey: workspaceSessionKeys.byWorkspace(workspaceId, hostId),
          });
        },
      });
    },
    [queryClient, hostId, workspaceId]
  );

  const { deleteSession, pendingSessionId: deletingSessionId } =
    useDeleteExecutionSession({
      hostId,
      scopeKey: JSON.stringify([
        hostId,
        location.href,
        workspaceId,
        sessionId,
        mode,
      ]),
      onDeleted: (target) => {
        if (sessionId !== target.sessionId) return;
        const nextSession = sessions.find(
          (item) => item.id !== target.sessionId
        );
        if (nextSession) onSelectSession?.(nextSession.id);
        else onStartNewSession?.();
      },
    });
  const handleDeleteSession = useCallback(
    (targetSessionId: string, currentName: string) => {
      if (!workspaceId) return;
      void deleteSession({
        sessionId: targetSessionId,
        workspaceId,
        title: currentName,
      });
    },
    [deleteSession, workspaceId]
  );
  const appNavigation = useAppNavigation();

  const actionsContext = useContext(ActionsContext);
  const { rightMainPanelMode, setRightMainPanelMode } =
    useWorkspacePanelState(workspaceId);

  const handleViewCode = useCallback(() => {
    setRightMainPanelMode(
      rightMainPanelMode === RIGHT_MAIN_PANEL_MODES.CHANGES
        ? null
        : RIGHT_MAIN_PANEL_MODES.CHANGES
    );
  }, [rightMainPanelMode, setRightMainPanelMode]);

  const handleOpenWorkspace = useCallback(() => {
    if (!workspaceId) return;
    appNavigation.goToWorkspace(workspaceId);
  }, [appNavigation, workspaceId]);

  // Get entries early to extract pending approval for scratch key
  const { entries } = useEntries();
  const tokenUsageInfo = useTokenUsage();
  const { timeline: canonicalTimeline } = useCanonicalAgentSession();
  const activeAgentRun = canonicalTimeline?.activeRun ?? null;
  const latestAgentRun = canonicalTimeline?.latestRun ?? null;
  const activeAgentRunState = activeAgentRun
    ? (activeAgentRun.timeline?.state ?? activeAgentRun.summary.state)
    : null;
  const canonicalAgentActionPolicy = useMemo(
    () =>
      deriveCanonicalAgentRunActionPolicy(
        activeAgentRunState,
        activeAgentRun?.timeline?.events ?? []
      ),
    [activeAgentRun, activeAgentRunState]
  );
  const latestAgentRunState = latestAgentRun
    ? (latestAgentRun.timeline?.state ?? latestAgentRun.summary.state)
    : null;
  const isAgentRunActive = Boolean(activeAgentRun);
  const isAgentRunCancelling = latestAgentRunState?.status === 'cancelling';

  // Extract user messages for turn navigation
  const userMessageTurns: TurnNavigationItem[] = useMemo(() => {
    let turnNumber = 0;
    return entries
      .filter(
        (entry) =>
          entry.type === 'NORMALIZED_ENTRY' &&
          entry.content.entry_type.type === 'user_message'
      )
      .map((entry) => {
        turnNumber++;
        return {
          patchKey: entry.patchKey,
          content:
            entry.type === 'NORMALIZED_ENTRY' ? entry.content.content : '',
          turnNumber,
        };
      });
  }, [entries]);

  // Execution state
  const {
    processes,
    stopExecution: stopStandaloneScripts,
    isStopping: isStandaloneScriptStopping,
  } = useWorkspaceExecution(workspaceId);
  const isStandaloneScriptActive = processes.some(
    (process) =>
      process.executor_action.typ.type === 'ScriptRequest' &&
      isExecutionProcessActive(process)
  );

  // Mobile layout — enables the enlarged approval footer + one-tap deny
  const isMobile = useIsMobile();

  // Pending approval for this workspace (shared with the mobile approval banner)
  const pendingApproval = useWorkspacePendingApproval();

  // Follow-up drafts are owned by their canonical session. Interaction
  // responses are transient and must never replace the session draft.
  const scratchId = isNewSessionMode ? workspaceId : sessionId;
  const [interactionMessage, setInteractionMessage] = useState('');

  useEffect(() => {
    setInteractionMessage('');
  }, [pendingApproval?.controlId]);

  // Get repos for file search
  const { repos } = useWorkspaceRepo(workspaceId);
  const workspaceQuery = useWorkspaceRecord(workspaceId);
  const workspace = explicitWorkspace ?? workspaceQuery.data;
  const repoIds = repos.map((r) => r.id);
  const resumeScopePath = useMemo(
    () =>
      resolveWorkspaceWorkingDirectory({
        containerRef: workspaceQuery.data?.container_ref,
        workingDir: session?.agent_working_dir,
        fallbackRepoName: repos.length === 1 ? repos[0].name : undefined,
      }),
    [repos, session?.agent_working_dir, workspaceQuery.data?.container_ref]
  );

  // Approval feedback context
  const feedbackContext = useApprovalFeedbackOptional();
  const isInFeedbackMode = !!feedbackContext?.activeApproval;

  // Message edit context
  const editContext = useMessageEditContext();
  const isInEditMode = editContext.isInEditMode;

  // Get todos from entries
  const { todos, inProgressTodo } = useTodos(entries);

  // Review comments context (optional - only available when ReviewProvider wraps this)
  const reviewContext = useReviewOptional();
  const reviewMarkdown = useMemo(
    () => reviewContext?.generateReviewMarkdown() ?? '',
    [reviewContext]
  );
  const hasReviewComments = (reviewContext?.comments.length ?? 0) > 0;

  // Approval mutation for approve/deny/answer actions
  const {
    approveAsync,
    denyAsync,
    answerAsync,
    isApproving,
    isDenying,
    isAnswering,
    denyError,
    answerError,
  } = useApprovalMutation();
  const { mutate: cancelAgentRun, isPending: isCancelAgentRunPending } =
    useMutation({
      mutationFn: (agentRunId: string) =>
        canonicalAgentControls.cancel(agentRunId, 'Cancelled by user.'),
      onError: (err) => {
        console.error('Failed to cancel AgentRun:', err);
      },
    });
  const isStopping =
    isCancelAgentRunPending ||
    isAgentRunCancelling ||
    isStandaloneScriptStopping;

  // Branch status for edit retry and conflict detection
  const { data: branchStatus } = useBranchStatus(workspaceId);

  // Derive conflict state from branch status
  const hasConflicts = useMemo(() => {
    return (
      branchStatus?.some((r) => (r.conflicted_files?.length ?? 0) > 0) ?? false
    );
  }, [branchStatus]);

  const conflictedFilesCount = useMemo(() => {
    return (
      branchStatus?.reduce(
        (sum, r) => sum + (r.conflicted_files?.length ?? 0),
        0
      ) ?? 0
    );
  }, [branchStatus]);

  // Get workspace branch for conflict resolution dialog
  const attemptBranch = workspace?.branch ?? null;

  // Find the first repo with conflicts (for the resolve dialog)
  const repoWithConflicts = useMemo(
    () =>
      branchStatus?.find(
        (r) => r.is_rebase_in_progress || (r.conflicted_files?.length ?? 0) > 0
      ),
    [branchStatus]
  );

  const handleResolveConflicts = useCallback(() => {
    if (!workspaceId || !repoWithConflicts) return;
    ResolveConflictsDialog.show({
      workspaceId,
      conflictOp: repoWithConflicts.conflict_op ?? 'rebase',
      sourceBranch: attemptBranch,
      targetBranch: repoWithConflicts.target_branch_name,
      conflictedFiles: repoWithConflicts.conflicted_files ?? [],
      repoName: repoWithConflicts.repo_name,
    });
  }, [workspaceId, repoWithConflicts, attemptBranch]);

  // User profiles and defaults are only initial values for unbound sessions.
  const { profiles, config, capabilities } = useUserSystem();

  // Message editor state
  const {
    localMessage,
    setLocalMessage,
    scratchData,
    isScratchLoading,
    hasInitialValue,
    saveToScratch,
    clearDraft,
    cancelDebouncedSave,
    handleMessageChange,
  } = useSessionMessageEditor({ scratchId });

  // Ref to access current message value for attachment handler
  const localMessageRef = useRef(localMessage);
  const draftScope = JSON.stringify([hostId, scratchId]);
  const draftOwnerRef = useRef({ scope: draftScope, active: true });
  if (draftOwnerRef.current.scope !== draftScope) {
    draftOwnerRef.current = { scope: draftScope, active: true };
  }
  useLayoutEffect(() => {
    const owner = draftOwnerRef.current;
    owner.active = true;
    return () => {
      owner.active = false;
    };
  }, [draftScope]);
  const draftIdentityRef = useRef({
    sessionId: draftScope,
    revision: 0,
  });
  if (draftIdentityRef.current.sessionId !== draftScope) {
    draftIdentityRef.current = { sessionId: draftScope, revision: 0 };
  }
  useEffect(() => {
    localMessageRef.current = localMessage;
  }, [localMessage]);

  // Attachment handling - insert markdown when attachments are uploaded
  const handleInsertMarkdown = useCallback(
    (markdown: string) => {
      const currentMessage = localMessageRef.current;
      const newMessage = currentMessage.trim()
        ? `${currentMessage}\n\n${markdown}`
        : markdown;
      draftIdentityRef.current.revision += 1;
      setLocalMessage(newMessage);
    },
    [setLocalMessage]
  );

  // Auto-paste component context from inspect mode
  const pendingComponentMarkdown = useInspectModeStore(
    (s) => s.pendingComponentMarkdown
  );
  const clearPendingComponentMarkdown = useInspectModeStore(
    (s) => s.clearPendingComponentMarkdown
  );

  useEffect(() => {
    if (presentation === 'full-page' && pendingComponentMarkdown) {
      handleInsertMarkdown(pendingComponentMarkdown);
      clearPendingComponentMarkdown();
    }
  }, [
    pendingComponentMarkdown,
    handleInsertMarkdown,
    clearPendingComponentMarkdown,
    presentation,
  ]);

  const { uploadFiles, localAttachments, clearUploadedAttachments } =
    useSessionAttachments(workspaceId, sessionId, handleInsertMarkdown);

  // The queue owns the submitted config until its durable run is created.
  const {
    isQueued,
    queuedMessage,
    queuedConfig,
    isQueueLoading,
    queueMessage,
    cancelQueue,
    refreshQueueStatus,
  } = useSessionQueueInteraction({ sessionId });

  // Unified executor + variant + model selector options resolution
  const {
    executorConfig,
    effectiveExecutor,
    selectedVariant,
    executorOptions,
    variantOptions,
    presetOptions,
    isConfigLoading,
    configurationLocked,
    configError,
    refetchConfig,
    needsExecutorSelection,
    setExecutor: setExecutor,
    setVariant: setSelectedVariant,
    setOverrides: setExecutorOverrides,
  } = useSessionExecutorConfig({
    sessionId,
    workspaceId,
    sessionExecutor: session?.executor,
    isNewSessionMode,
    preferredExecutorConfig,
    isScratchLoading: mode !== 'placeholder' && isScratchLoading,
    profiles,
    scratchConfig: isQueued ? queuedConfig : scratchData?.executor_config,
    configExecutorProfile: config?.executor_profile,
    hiddenAgents: config?.hidden_agents,
    onPersist: (cfg) => {
      draftIdentityRef.current.revision += 1;
      void saveToScratch(localMessageRef.current, cfg);
    },
  });
  const liveExecutorConfigRef = useRef({
    scope: draftScope,
    config: executorConfig,
  });
  if (liveExecutorConfigRef.current.scope !== draftScope || executorConfig) {
    // A refetch temporarily gates sending with null; retain the latest choice
    // only for acknowledgement of this same draft, never for another session.
    liveExecutorConfigRef.current = {
      scope: draftScope,
      config: executorConfig,
    };
  }

  // Queue launches, retries and other tabs can create a new attempt without
  // passing through this composer's send acknowledgement.
  const latestRunAttemptId = latestAgentRunState?.last_run_attempt_id;
  useEffect(() => {
    if (sessionId && latestRunAttemptId) void refetchConfig();
  }, [sessionId, latestRunAttemptId, refetchConfig]);
  const providerPolicy = useAgentProviderPolicy(effectiveExecutor);
  const [stagedResumeSession, setStagedResumeSession] =
    useState<ResumableAgentSession | null>(null);

  const handleExecutorChange = useCallback(
    (executor: BaseCodingAgent) => {
      setStagedResumeSession(null);
      setExecutor(executor);
    },
    [setExecutor]
  );

  useEffect(() => {
    setStagedResumeSession(null);
  }, [workspaceId, sessionId, effectiveExecutor]);

  const supportsContextUsage =
    !!effectiveExecutor &&
    capabilities?.[effectiveExecutor]?.includes(
      BaseAgentCapability.CONTEXT_USAGE
    );

  // Navigate to agent settings to customise variants
  const handleCustomise = () => {
    openAgentCenter();
  };

  const [selectedSkills, setSelectedSkills] = useState<SelectedSkill[]>([]);

  useEffect(() => {
    setSelectedSkills([]);
  }, [sessionId, workspaceId]);

  // Send actions
  const {
    send,
    isSending,
    error: sendError,
    clearError,
  } = useSessionSend({
    sessionId,
    workspaceId,
    isNewSessionMode,
    onSelectSession,
    executorConfig,
  });

  const handleSend = useCallback(async () => {
    if (!executorConfig) return;
    const owner = draftOwnerRef.current;
    const submittedMessage = localMessage;
    const submission = snapshotSessionDraft({
      sessionId: draftScope,
      text: submittedMessage,
      revision: draftIdentityRef.current.revision,
    });
    const { prompt, isSlashCommand } = buildAgentPrompt(submittedMessage, [
      reviewMarkdown,
    ]);

    onScrollToBottom('auto');

    const success = await send(prompt, isSlashCommand ? [] : selectedSkills, {
      resumeSessionId: stagedResumeSession?.agent_session_id,
      resumeScopePath: resumeScopePath,
    });
    if (success) {
      if (!owner.active || draftOwnerRef.current !== owner) return;
      // Refresh the canonical snapshot before deleting the submitted draft.
      if (sessionId) await refetchConfig();
      if (!owner.active || draftOwnerRef.current !== owner) return;
      if (draftIdentityRef.current.sessionId !== submission.sessionId) return;
      cancelDebouncedSave();
      const currentMessage = localMessageRef.current;
      if (
        isSessionDraftSubmissionCurrent(
          draftIdentityRef.current.sessionId,
          currentMessage,
          draftIdentityRef.current.revision,
          submission
        )
      ) {
        draftIdentityRef.current.revision += 1;
        setLocalMessage('');
        await clearDraft();
      } else if (
        liveExecutorConfigRef.current.config &&
        draftIdentityRef.current.sessionId === submission.sessionId
      ) {
        await saveToScratch(
          currentMessage,
          liveExecutorConfigRef.current.config
        );
      }
      setSelectedSkills([]);
      setStagedResumeSession(null);
      clearUploadedAttachments();
      if (!isSlashCommand) {
        reviewContext?.clearComments();
      }
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          onScrollToBottom('auto');
        });
      });
    }
  }, [
    onScrollToBottom,
    send,
    localMessage,
    draftScope,
    reviewMarkdown,
    selectedSkills,
    stagedResumeSession?.agent_session_id,
    resumeScopePath,
    cancelDebouncedSave,
    setLocalMessage,
    executorConfig,
    saveToScratch,
    clearUploadedAttachments,
    clearDraft,
    reviewContext,
    sessionId,
    refetchConfig,
  ]);

  // Track canonical run discovery for queue refresh.
  const agentRunCount = canonicalTimeline?.runs.length ?? 0;
  const prevAgentRunCountRef = useRef(agentRunCount);

  // Refresh queue status when a canonical run stops or a new run appears.
  useEffect(() => {
    const prevCount = prevAgentRunCountRef.current;
    prevAgentRunCountRef.current = agentRunCount;

    if (!workspaceId) return;

    if (!isAgentRunActive) {
      refreshQueueStatus();
      return;
    }

    if (agentRunCount > prevCount) {
      refreshQueueStatus();
    }
  }, [agentRunCount, isAgentRunActive, workspaceId, refreshQueueStatus]);

  // Queue message handler
  const handleQueueMessage = useCallback(async () => {
    // Allow queueing if there's a message OR review comments, and we have a config
    if ((!localMessage.trim() && !reviewMarkdown) || !executorConfig) return;
    const owner = draftOwnerRef.current;

    const submittedMessage = localMessage;
    const submission = snapshotSessionDraft({
      sessionId: draftScope,
      text: submittedMessage,
      revision: draftIdentityRef.current.revision,
    });
    const { prompt } = buildAgentPrompt(submittedMessage, [reviewMarkdown]);

    cancelDebouncedSave();
    await saveToScratch(submittedMessage, executorConfig);
    await queueMessage(prompt, executorConfig, selectedSkills);

    if (!owner.active || draftOwnerRef.current !== owner) return;
    if (draftIdentityRef.current.sessionId !== submission.sessionId) return;
    const currentMessage = localMessageRef.current;
    if (
      isSessionDraftSubmissionCurrent(
        draftIdentityRef.current.sessionId,
        currentMessage,
        draftIdentityRef.current.revision,
        submission
      )
    ) {
      draftIdentityRef.current.revision += 1;
      setLocalMessage('');
      await clearDraft();
    } else if (liveExecutorConfigRef.current.config) {
      await saveToScratch(currentMessage, liveExecutorConfigRef.current.config);
    }
    setSelectedSkills([]);
    clearUploadedAttachments();
    reviewContext?.clearComments();
  }, [
    localMessage,
    draftScope,
    reviewMarkdown,
    executorConfig,
    selectedSkills,
    queueMessage,
    cancelDebouncedSave,
    saveToScratch,
    clearDraft,
    setLocalMessage,
    clearUploadedAttachments,
    reviewContext,
  ]);

  // Editor change handler
  const handleEditorChange = useCallback(
    (value: string) => {
      if (pendingApproval) {
        setInteractionMessage(value);
        if (sendError) clearError();
        return;
      }
      draftIdentityRef.current.revision += 1;
      if (isQueued) cancelQueue();
      if (executorConfig) {
        handleMessageChange(value, executorConfig);
      } else {
        setLocalMessage(value);
      }
      if (sendError) clearError();
    },
    [
      isQueued,
      cancelQueue,
      handleMessageChange,
      executorConfig,
      sendError,
      clearError,
      setLocalMessage,
      pendingApproval,
    ]
  );

  // Handle feedback submission
  const handleSubmitFeedback = useCallback(async () => {
    if (!feedbackContext || !localMessage.trim()) return;
    try {
      await feedbackContext.submitFeedback(localMessage);
      cancelDebouncedSave();
      setLocalMessage('');
      await clearDraft();
    } catch {
      // Error is handled in context
    }
  }, [
    feedbackContext,
    localMessage,
    cancelDebouncedSave,
    setLocalMessage,
    clearDraft,
  ]);

  // Handle cancel feedback mode
  const handleCancelFeedback = useCallback(() => {
    feedbackContext?.exitFeedbackMode();
  }, [feedbackContext]);

  // Handle cancel queue - restore message to editor
  const handleCancelQueue = useCallback(async () => {
    if (queuedMessage) {
      setLocalMessage(queuedMessage);
    }
    if (queuedConfig) {
      setExecutorOverrides(queuedConfig);
    }
    await cancelQueue();
  }, [
    queuedMessage,
    queuedConfig,
    setLocalMessage,
    setExecutorOverrides,
    cancelQueue,
  ]);

  // Message edit retry mutation
  const editRetryMutation = useMessageEditRetry(sessionId ?? '', () => {
    // On success, clear edit mode and reset editor
    editContext.cancelEdit();
    cancelDebouncedSave();
    setLocalMessage('');
  });

  const areAttachmentInputsDisabled =
    mode === 'placeholder' ||
    isQueued ||
    isSending ||
    isStopping ||
    !!feedbackContext?.isSubmitting ||
    editRetryMutation.isPending ||
    isApproving ||
    isDenying ||
    isAnswering;

  const onDrop = useCallback(
    (acceptedFiles: File[]) => {
      if (acceptedFiles.length > 0) {
        uploadFiles(acceptedFiles);
      }
    },
    [uploadFiles]
  );

  const { getRootProps, getInputProps, isDragActive } = useDropzone({
    onDrop,
    disabled: areAttachmentInputsDisabled,
    noClick: true,
    noKeyboard: true,
  });

  // Handle edit submission
  const handleSubmitEdit = useCallback(async () => {
    if (!editContext.activeEdit || !localMessage.trim() || !executorConfig)
      return;
    editRetryMutation.mutate({
      message: localMessage,
      executorConfig,
      executionProcessId: editContext.activeEdit.processId,
      branchStatus,
      processes,
    });
  }, [
    editContext.activeEdit,
    localMessage,
    executorConfig,
    branchStatus,
    processes,
    editRetryMutation,
  ]);

  // Handle cancel edit mode
  const handleCancelEdit = useCallback(() => {
    editContext.cancelEdit();
    setLocalMessage('');
  }, [editContext, setLocalMessage]);

  // Populate editor with original message when entering edit mode
  const prevEditRef = useRef(editContext.activeEdit);
  useEffect(() => {
    if (editContext.activeEdit && !prevEditRef.current) {
      // Just entered edit mode - populate with original message
      setLocalMessage(editContext.activeEdit.originalMessage);
    }
    prevEditRef.current = editContext.activeEdit;
  }, [editContext.activeEdit, setLocalMessage]);

  // Handle inserting PR comments into the message editor
  const handleInsertPrComments = useCallback(async () => {
    if (!workspaceId) return;
    const repoId = repos[0]?.id;
    if (!repoId) return;

    const result = await PrCommentsDialog.show({
      workspaceId: workspaceId,
      repoId,
    });
    if (result.comments.length > 0) {
      const markdownBlocks = result.comments.map((comment) => {
        const payload: NormalizedComment = {
          id:
            comment.comment_type === 'general'
              ? comment.id
              : comment.id.toString(),
          comment_type: comment.comment_type,
          author: comment.author,
          body: comment.body,
          created_at: comment.created_at,
          url: comment.url,
          ...(comment.comment_type === 'review' && {
            path: comment.path,
            line: comment.line != null ? Number(comment.line) : null,
            diff_hunk: comment.diff_hunk,
          }),
        };
        return '```gh-comment\n' + JSON.stringify(payload, null, 2) + '\n```';
      });
      handleInsertMarkdown(markdownBlocks.join('\n\n'));
    }
  }, [workspaceId, repos, handleInsertMarkdown]);

  // The composer has one page-only action. Its explicit target is the same
  // workspace as the session, independent of the page's selected workspace.
  const toolbarActionItems = useMemo(
    () =>
      presentation !== 'embedded' && workspaceId && workspace && actionsContext
        ? [
            {
              id: Actions.StartReview.id,
              icon: Actions.StartReview.icon,
              label: t('common:actions.startReview', {
                defaultValue: 'Start review',
              }),
              tooltip: t('common:actions.startReviewDescription', {
                defaultValue: 'Review changes with agent',
              }),
              onClick: () =>
                void actionsContext.executeAction(
                  Actions.StartReview,
                  workspaceId
                ),
            },
          ]
        : [],
    [presentation, workspaceId, workspace, actionsContext, t]
  );

  // Handle approve action
  const handleApprove = useCallback(async () => {
    if (pendingApproval?.kind !== 'approval') return;

    // Exit feedback mode if active
    feedbackContext?.exitFeedbackMode();

    try {
      await approveAsync({
        approvalId: pendingApproval.approvalId,
        agentRunId: pendingApproval.agentRunId,
      });

      // Invalidate workspace summary cache to update sidebar
      queryClient.invalidateQueries({ queryKey: workspaceSummaryKeys.all });
      onScrollToBottom();
    } catch {
      // Error is handled by mutation
    }
  }, [
    pendingApproval,
    feedbackContext,
    approveAsync,
    queryClient,
    onScrollToBottom,
  ]);

  // Handle request changes (deny with feedback)
  const handleRequestChanges = useCallback(async () => {
    if (pendingApproval?.kind !== 'approval' || !interactionMessage.trim())
      return;

    try {
      await denyAsync({
        approvalId: pendingApproval.approvalId,
        agentRunId: pendingApproval.agentRunId,
        reason: interactionMessage.trim(),
      });
      setInteractionMessage('');

      // Invalidate workspace summary cache to update sidebar
      queryClient.invalidateQueries({ queryKey: workspaceSummaryKeys.all });
      onScrollToBottom();
    } catch {
      // Error is handled by mutation
    }
  }, [
    pendingApproval,
    interactionMessage,
    denyAsync,
    queryClient,
    onScrollToBottom,
  ]);

  // Handle one-tap deny (reason optional): sends the typed text as the reason
  // when present, otherwise a default reason. Mirrors handleRequestChanges but
  // does not require the composer to be non-empty — this powers the mobile
  // approval footer's dedicated Deny button.
  const handleDeny = useCallback(async () => {
    if (pendingApproval?.kind !== 'approval') return;

    try {
      await denyAsync({
        approvalId: pendingApproval.approvalId,
        agentRunId: pendingApproval.agentRunId,
        reason:
          interactionMessage.trim() || 'User denied this tool use request.',
      });
      setInteractionMessage('');

      // Invalidate workspace summary cache to update sidebar
      queryClient.invalidateQueries({ queryKey: workspaceSummaryKeys.all });
      onScrollToBottom();
    } catch {
      // Error is handled by mutation
    }
  }, [
    pendingApproval,
    interactionMessage,
    denyAsync,
    queryClient,
    onScrollToBottom,
  ]);

  // Handle AskUserQuestion answer submission
  const handleAnswerQuestion = useCallback(
    async (answers: Array<{ question: string; answer: string[] }>) => {
      if (pendingApproval?.kind !== 'input') return;

      try {
        await answerAsync({
          agentRunId: pendingApproval.agentRunId,
          inputId: pendingApproval.inputId,
          answers,
        });
        queryClient.invalidateQueries({
          queryKey: workspaceSummaryKeys.all,
        });
        onScrollToBottom();
      } catch {
        // Error is handled by mutation
      }
    },
    [pendingApproval, answerAsync, queryClient, onScrollToBottom]
  );

  const isApprovalTimedOut = false;

  const handleStop = useCallback(() => {
    if (activeAgentRun) {
      if (
        !canonicalAgentActionPolicy.cancel.allowed ||
        isCancelAgentRunPending
      ) {
        return;
      }
      cancelAgentRun(activeAgentRun.summary.agent_run_id);
      return;
    }

    if (isStandaloneScriptActive && !isStandaloneScriptStopping) {
      void stopStandaloneScripts();
    }
  }, [
    activeAgentRun,
    cancelAgentRun,
    canonicalAgentActionPolicy.cancel.allowed,
    isCancelAgentRunPending,
    isStandaloneScriptActive,
    isStandaloneScriptStopping,
    stopStandaloneScripts,
  ]);

  const composerMessage = pendingApproval ? interactionMessage : localMessage;
  const hasMessageContent =
    composerMessage.trim().length > 0 || reviewMarkdown.length > 0;
  const runtimeActionPolicy = useMemo(
    () =>
      deriveRuntimeActionPolicy({
        hasContent: hasMessageContent,
        hasWorkspace: !!workspaceId,
        hasSession: !!sessionId,
        hasExecutor: !!executorConfig,
        isNewSessionMode,
        hasPriorAgentRun: (canonicalTimeline?.runs.length ?? 0) > 0,
        isAgentRunActive,
        isAgentRunCancelling,
        isLatestAgentRunTerminal: canonicalTimeline?.isTerminal ?? false,
        isStandaloneScriptActive,
        canCancelAgentRun: canonicalAgentActionPolicy.cancel.allowed,
        canResolveApproval: canonicalAgentActionPolicy.resolve_approval.allowed,
        canSubmitInput: canonicalAgentActionPolicy.submit_input.allowed,
        isSending,
        isStopping,
        isQueueLoading,
        isQueued,
        hasPendingApproval: pendingApproval?.kind === 'approval',
        hasPendingQuestion: pendingApproval?.kind === 'input',
        isApprovalTimedOut,
        isApprovalSubmitting: isApproving || isDenying,
        isQuestionSubmitting: isAnswering,
        providerPolicy,
      }),
    [
      hasMessageContent,
      workspaceId,
      sessionId,
      executorConfig,
      isNewSessionMode,
      canonicalTimeline?.runs.length,
      canonicalTimeline?.isTerminal,
      isAgentRunActive,
      isAgentRunCancelling,
      canonicalAgentActionPolicy.cancel.allowed,
      canonicalAgentActionPolicy.resolve_approval.allowed,
      canonicalAgentActionPolicy.submit_input.allowed,
      isStandaloneScriptActive,
      isSending,
      isStopping,
      isQueueLoading,
      isQueued,
      pendingApproval,
      isApprovalTimedOut,
      isApproving,
      isDenying,
      isAnswering,
      providerPolicy,
    ]
  );

  const status = computeExecutionStatus({
    isInFeedbackMode,
    isInEditMode,
    isStopping,
    isQueueLoading,
    isSendingFollowUp: isSending,
    isQueued,
    isAgentRunActive: isAgentRunActive || isStandaloneScriptActive,
  });

  // During loading, render with empty editor to preserve container UI
  // In approval mode, don't show queued message - it's for follow-up, not approval response
  const editorValue = useMemo(() => {
    if (isScratchLoading || !hasInitialValue) return '';
    if (pendingApproval) return interactionMessage;
    return queuedMessage ?? localMessage;
  }, [
    isScratchLoading,
    hasInitialValue,
    pendingApproval,
    interactionMessage,
    queuedMessage,
    localMessage,
  ]);

  const renderEditor = useCallback(
    ({
      focusKey,
      placeholder,
      value,
      onChange,
      onCmdEnter,
      disabled,
      repoIds,
      executor,
      onPasteFiles,
      localAttachments,
    }: SessionChatBoxEditorRenderProps<BaseCodingAgent>) => (
      <WYSIWYGEditor
        key={focusKey}
        placeholder={placeholder}
        value={value}
        onChange={onChange}
        onCmdEnter={onCmdEnter}
        disabled={disabled}
        className={
          presentation === 'embedded'
            ? 'min-h-8 max-h-[min(160px,25vh)] overflow-y-auto'
            : 'min-h-double max-h-[50vh] overflow-y-auto'
        }
        repoIds={repoIds}
        executor={executor}
        selectedSkills={selectedSkills}
        onSelectedSkillsChange={setSelectedSkills}
        workspaceId={workspaceId}
        sessionId={sessionId}
        diffPaths={diffPaths ?? EMPTY_DIFF_PATHS}
        autoFocus
        onPasteFiles={onPasteFiles}
        localAttachments={localAttachments}
        sendShortcut={config?.send_message_shortcut}
      />
    ),
    [
      config?.send_message_shortcut,
      selectedSkills,
      sessionId,
      workspaceId,
      presentation,
      diffPaths,
    ]
  );

  const resumePickerNode =
    resumeScopePath && effectiveExecutor ? (
      <AgentSessionResumePicker
        scopePath={resumeScopePath}
        executor={effectiveExecutor}
        selectedSessionId={stagedResumeSession?.agent_session_id}
        disabled={isSending || isStopping || isAgentRunActive}
        onSelect={setStagedResumeSession}
      />
    ) : undefined;

  const modelSelectorNode = isConfigLoading ? (
    <span className="text-low text-sm" role="status">
      {t('common:sessionConfig.loading', 'Restoring session configuration…')}
    </span>
  ) : configError ? (
    <button
      type="button"
      className="text-error text-sm"
      onClick={() => void refetchConfig()}
    >
      {t('common:sessionConfig.retry', 'Retry loading session configuration')}
    </button>
  ) : effectiveExecutor || stagedResumeSession ? (
    <>
      {effectiveExecutor && (
        <ModelSelectorContainer
          key={`${hostId ?? 'local'}:${sessionId ?? workspaceId}`}
          agent={effectiveExecutor}
          workspaceId={workspaceId}
          sessionId={sessionId}
          onAdvancedSettings={handleCustomise}
          presets={variantOptions}
          selectedPreset={selectedVariant}
          onPresetSelect={setSelectedVariant}
          onOverrideChange={setExecutorOverrides}
          executorConfig={executorConfig}
          presetOptions={presetOptions}
          readOnly={configurationLocked}
        />
      )}
      {stagedResumeSession && (
        <AgentSessionResumeChip
          session={stagedResumeSession}
          onClear={() => setStagedResumeSession(null)}
        />
      )}
    </>
  ) : undefined;

  // In placeholder mode, render a disabled version to maintain visual structure
  if (mode === 'placeholder') {
    return (
      <SessionChatBox<BaseCodingAgent>
        status="idle"
        renderEditor={renderEditor}
        repoIds={repoIds}
        tokenUsageInfo={tokenUsageInfo}
        supportsContextUsage={false}
        formatExecutorLabel={toPrettyCase}
        formatSessionDate={(createdAt) =>
          formatDateShortWithTime(
            createdAt instanceof Date ? createdAt.toISOString() : createdAt
          )
        }
        renderAgentIcon={(executor, className) => (
          <AgentIcon
            agent={executor as BaseCodingAgent | null | undefined}
            className={className}
          />
        )}
        editor={{
          value: '',
          onChange: () => {},
        }}
        actions={{
          onSend: () => {},
          onQueue: () => {},
          onCancelQueue: () => {},
          onStop: () => {},
          onPasteFiles: () => {},
        }}
        session={{
          sessions: [],
          selectedSessionId: undefined,
          onSelectSession: () => {},
          isNewSessionMode: false,
          onNewSession: undefined,
        }}
        stats={{
          filesChanged: 0,
          linesAdded: 0,
          linesRemoved: 0,
        }}
        onViewCode={disableViewCode ? undefined : handleViewCode}
      />
    );
  }

  return (
    <SessionChatBox<BaseCodingAgent>
      status={status}
      compact={presentation === 'embedded'}
      showSessionControls={presentation !== 'embedded'}
      isMobile={isMobile}
      onViewCode={disableViewCode ? undefined : handleViewCode}
      onOpenWorkspace={
        showOpenWorkspaceButton && workspaceId ? handleOpenWorkspace : undefined
      }
      onScrollToPreviousMessage={onScrollToPreviousMessage}
      userMessageTurns={userMessageTurns}
      onScrollToUserMessage={onScrollToUserMessage}
      getActiveTurnPatchKey={getActiveTurnPatchKey}
      renderEditor={renderEditor}
      repoIds={repoIds}
      tokenUsageInfo={tokenUsageInfo}
      supportsContextUsage={supportsContextUsage}
      formatExecutorLabel={toPrettyCase}
      formatSessionDate={(createdAt) =>
        formatDateShortWithTime(
          createdAt instanceof Date ? createdAt.toISOString() : createdAt
        )
      }
      renderAgentIcon={(executor, className) => (
        <AgentIcon
          agent={executor as BaseCodingAgent | null | undefined}
          className={className}
        />
      )}
      editor={{
        value: editorValue,
        onChange: handleEditorChange,
      }}
      actions={{
        onSend: handleSend,
        onQueue: handleQueueMessage,
        onCancelQueue: handleCancelQueue,
        onStop: handleStop,
        onPasteFiles: uploadFiles,
      }}
      actionPolicy={runtimeActionPolicy}
      session={{
        sessions,
        selectedSessionId: sessionId,
        onSelectSession: onSelectSession ?? (() => {}),
        isInitialSendMode: isNewSessionMode,
        isNewSessionMode: needsExecutorSelection,
        onNewSession:
          presentation === 'embedded' ? undefined : onStartNewSession,
        onRenameSession:
          presentation === 'embedded' ? undefined : handleRenameSession,
        onDeleteSession:
          mode === 'existing-session' && presentation !== 'embedded'
            ? handleDeleteSession
            : undefined,
        deletingSessionId,
      }}
      toolbarActions={{
        items: toolbarActionItems,
      }}
      onPrCommentClick={
        presentation !== 'embedded' &&
        branchStatus?.some((repo) =>
          repo.merges?.some(
            (merge) => merge.type === 'pr' && merge.pr_info.status === 'open'
          )
        )
          ? handleInsertPrComments
          : undefined
      }
      stats={{
        filesChanged,
        linesAdded,
        linesRemoved,
        diffStatsUnavailable,
        hasConflicts,
        conflictedFilesCount,
        onResolveConflicts: handleResolveConflicts,
      }}
      error={configError?.message ?? sendError}
      agent={effectiveExecutor}
      todos={todos}
      inProgressTodo={inProgressTodo}
      executor={
        needsExecutorSelection
          ? {
              selected: effectiveExecutor,
              options: executorOptions,
              onChange: handleExecutorChange,
              afterSelector: resumePickerNode,
            }
          : undefined
      }
      feedbackMode={
        feedbackContext
          ? {
              isActive: isInFeedbackMode,
              onSubmitFeedback: handleSubmitFeedback,
              onCancel: handleCancelFeedback,
              isSubmitting: feedbackContext.isSubmitting,
              error: feedbackContext.error,
              isTimedOut: feedbackContext.isTimedOut,
            }
          : undefined
      }
      approvalMode={
        pendingApproval?.kind === 'approval'
          ? {
              isActive: true,
              onApprove: handleApprove,
              onRequestChanges: handleRequestChanges,
              onDeny: handleDeny,
              isSubmitting: isApproving || isDenying,
              isTimedOut: isApprovalTimedOut,
              error: denyError?.message ?? null,
            }
          : undefined
      }
      askQuestionMode={
        pendingApproval?.kind === 'input'
          ? {
              isActive: true,
              questions: pendingApproval.questions,
              onSubmitAnswers: handleAnswerQuestion,
              isSubmitting: isAnswering,
              isTimedOut: isApprovalTimedOut,
              error: answerError?.message ?? null,
            }
          : undefined
      }
      editMode={{
        isActive: isInEditMode,
        onSubmitEdit: handleSubmitEdit,
        onCancel: handleCancelEdit,
        isSubmitting: editRetryMutation.isPending,
      }}
      reviewComments={
        hasReviewComments && reviewContext
          ? {
              count: reviewContext.comments.length,
              previewMarkdown: reviewMarkdown,
              onClear: reviewContext.clearComments,
            }
          : undefined
      }
      localAttachments={localAttachments}
      dropzone={{ getRootProps, getInputProps, isDragActive }}
      modelSelector={modelSelectorNode}
      resumeSelector={
        presentation === 'embedded' ||
        needsExecutorSelection ||
        configurationLocked
          ? undefined
          : resumePickerNode
      }
    />
  );
}
