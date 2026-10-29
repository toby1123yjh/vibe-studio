import { type ChangeEvent, type ReactNode, useRef } from 'react';
import {
  type Icon,
  PaperclipIcon,
  CheckIcon,
  ClockIcon,
  XIcon,
  PlusIcon,
  SquareIcon,
  ChatCircleIcon,
  DotsThreeIcon,
  TrashIcon,
  WarningIcon,
  ClockCounterClockwiseIcon,
  ArrowsOutIcon,
  GithubLogoIcon,
  PencilSimpleIcon,
} from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import { cn } from '../lib/cn';
import { ChatBoxBase, VisualVariant, type DropzoneProps } from './ChatBoxBase';
import { type EditorProps, type ExecutorProps } from './CreateChatBox';
import type { AskUserQuestionItem, QuestionAnswer } from 'shared/types';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from './Dropdown';
import { PrimaryButton } from './PrimaryButton';
import { ChatComposerActivity } from './ChatComposerActivity';
import type { LocalAttachmentMetadata } from './WorkspaceContext';
import { ToolbarDropdown, ToolbarIconButton } from './Toolbar';
import { ContextUsageGauge, type ContextUsageInfo } from './ContextUsageGauge';
import { TodoProgressPopup, type TodoProgressItem } from './TodoProgressPopup';
import {
  AskUserQuestionBanner,
  type AskUserQuestionBannerHandle,
} from './AskUserQuestionBanner';
import {
  TurnNavigationPopup,
  type TurnNavigationItem,
} from './TurnNavigationPopup';

// Status enum - single source of truth for execution state
export type ExecutionStatus =
  | 'idle'
  | 'sending'
  | 'running'
  | 'queued'
  | 'stopping'
  | 'queue-loading'
  | 'feedback'
  | 'edit';

export type SessionRuntimeAction =
  | 'send_initial'
  | 'send_follow_up'
  | 'queue_follow_up'
  | 'cancel_queue'
  | 'stop'
  | 'approve'
  | 'request_changes'
  | 'answer_question'
  | 'retry'
  | 'resume';

export interface SessionRuntimeActionDecision {
  action: SessionRuntimeAction;
  allowed: boolean;
  reason: string | null;
}

export type SessionRuntimeActionPolicy = Record<
  SessionRuntimeAction,
  SessionRuntimeActionDecision
>;

const SESSION_RUNTIME_ACTION_BLOCKED_REASON_LABELS: Record<string, string> = {
  no_content: 'Add a message before sending.',
  no_workspace: 'Select a workspace first.',
  no_session: 'Select or create a session first.',
  no_executor: 'Select an agent before sending.',
  provider_not_ready: 'The selected agent is not ready.',
  provider_capability_missing:
    'The selected agent does not support this action.',
  runtime_starting: 'The agent action is starting. Wait for it to finish.',
  runtime_busy: 'The agent is still running. Queue a follow-up instead.',
  runtime_cancelling: 'The agent is stopping. Wait for cancellation to finish.',
  runtime_terminal: 'The last run ended and cannot continue from here.',
  approval_timed_out: 'The approval request timed out.',
  approval_required: 'Respond to the pending approval first.',
  question_required: 'Answer the pending question first.',
  queue_already_present: 'A follow-up is already queued.',
  queue_empty: 'There is no queued follow-up to cancel.',
  unknown_runtime: 'This action is not available in the current runtime state.',
};

export function getSessionRuntimeActionBlockedReasonLabel(
  reason: string | null | undefined
) {
  if (!reason) return null;
  return (
    SESSION_RUNTIME_ACTION_BLOCKED_REASON_LABELS[reason] ??
    'This action is not available right now.'
  );
}

function getBlockedActionMessage(
  actionLabel: string,
  decision: SessionRuntimeActionDecision
) {
  if (decision.allowed) return undefined;
  const reasonLabel = getSessionRuntimeActionBlockedReasonLabel(
    decision.reason
  );
  return reasonLabel ? `${actionLabel} unavailable: ${reasonLabel}` : undefined;
}

function getActionButtonA11yProps(
  actionLabel: string,
  decision: SessionRuntimeActionDecision
) {
  const blockedMessage = getBlockedActionMessage(actionLabel, decision);
  if (blockedMessage) {
    return {
      title: blockedMessage,
      'aria-label': blockedMessage,
    };
  }
  return {
    'aria-label': actionLabel,
  };
}

const INLINE_BLOCKED_HINT_EXCLUDED_REASONS = new Set([
  'no_content',
  'queue_empty',
  // Action buttons already communicate these transient states. Repeating them
  // as a warning can wrap the toolbar and move the editor while sending.
  'runtime_starting',
  'runtime_busy',
  'runtime_cancelling',
]);

interface ActionsProps {
  onSend: (markdown: string) => void;
  onQueue: (markdown: string) => void;
  onCancelQueue: () => void;
  onStop: () => void;
  onPasteFiles: (files: File[]) => void;
}

export interface SessionOption<TExecutor extends string = string> {
  id: string;
  name?: string | null;
  created_at: string | Date;
  executor?: TExecutor | string | null;
}

interface SessionProps<TExecutor extends string = string> {
  sessions: SessionOption<TExecutor>[];
  selectedSessionId?: string;
  onSelectSession: (sessionId: string) => void;
  /** True when Send creates a new session instead of posting a follow-up. */
  isInitialSendMode?: boolean;
  isNewSessionMode?: boolean;
  onNewSession?: () => void;
  onRenameSession?: (sessionId: string, currentName: string) => void;
  onDeleteSession?: (sessionId: string, currentName: string) => void;
  deletingSessionId?: string | null;
}

export interface SessionToolbarActionItem {
  id: string;
  icon: Icon;
  label: string;
  tooltip?: string;
  onClick: () => void;
  disabled?: boolean;
  requiresIdle?: boolean;
}

interface ToolbarActionsProps {
  items: SessionToolbarActionItem[];
}

interface StatsProps {
  filesChanged?: number;
  linesAdded?: number;
  linesRemoved?: number;
  diffStatsUnavailable?: boolean;
  hasConflicts?: boolean;
  conflictedFilesCount?: number;
  onResolveConflicts?: () => void;
}

interface FeedbackModeProps {
  isActive: boolean;
  onSubmitFeedback: (markdown: string) => void;
  onCancel: () => void;
  isSubmitting: boolean;
  error?: string | null;
  isTimedOut: boolean;
}

interface EditModeProps {
  isActive: boolean;
  onSubmitEdit: (markdown: string) => void;
  onCancel: () => void;
  isSubmitting: boolean;
}

interface ApprovalModeProps {
  isActive: boolean;
  onApprove: () => void;
  onRequestChanges: (markdown: string) => void;
  /** One-tap deny (reason optional). Used by the mobile approval footer. */
  onDeny: () => void;
  isSubmitting: boolean;
  isTimedOut: boolean;
  error?: string | null;
}

interface AskQuestionModeProps {
  isActive: boolean;
  questions: AskUserQuestionItem[];
  onSubmitAnswers: (answers: QuestionAnswer[]) => void;
  isSubmitting: boolean;
  isTimedOut: boolean;
  error?: string | null;
}

interface ReviewCommentsProps {
  /** Number of review comments */
  count: number;
  /** Preview markdown of the comments */
  previewMarkdown: string;
  /** Clear all comments */
  onClear: () => void;
}

export interface SessionChatBoxEditorRenderProps<
  TExecutor extends string = string,
> {
  focusKey: string;
  placeholder: string;
  value: string;
  onChange: (value: string) => void;
  onCmdEnter: (markdown: string) => void;
  disabled: boolean;
  repoIds?: string[];
  executor: TExecutor | null;
  onPasteFiles: (files: File[]) => void;
  localAttachments?: LocalAttachmentMetadata[];
}

interface SessionChatBoxProps<TExecutor extends string = string> {
  status: ExecutionStatus;
  editor: EditorProps;
  renderEditor: (
    props: SessionChatBoxEditorRenderProps<TExecutor>
  ) => ReactNode;
  actions: ActionsProps;
  actionPolicy?: SessionRuntimeActionPolicy;
  /** Re-evaluate policy against the editor snapshot before a content action. */
  getActionPolicy?: (markdown: string) => SessionRuntimeActionPolicy;
  isConfigLoading?: boolean;
  session: SessionProps<TExecutor>;
  stats?: StatsProps;
  feedbackMode?: FeedbackModeProps;
  editMode?: EditModeProps;
  approvalMode?: ApprovalModeProps;
  askQuestionMode?: AskQuestionModeProps;
  reviewComments?: ReviewCommentsProps;
  toolbarActions?: ToolbarActionsProps;
  modelSelector?: ReactNode;
  resumeSelector?: ReactNode;
  error?: string | null;
  repoIds?: string[];
  agent?: TExecutor | null;
  agentBrand?: string;
  executor?: ExecutorProps<TExecutor>;
  formatExecutorLabel?: (executor: TExecutor) => string;
  emptyExecutorLabel?: string;
  renderAgentIcon?: (
    executor: TExecutor | string | null | undefined,
    className?: string
  ) => ReactNode;
  formatSessionDate?: (createdAt: string | Date) => string;
  todos?: TodoProgressItem[];
  inProgressTodo?: TodoProgressItem | null;
  localAttachments?: LocalAttachmentMetadata[];
  onPrCommentClick?: () => void;
  onViewCode?: () => void;
  onOpenWorkspace?: () => void;
  onScrollToPreviousMessage?: () => void;
  userMessageTurns?: TurnNavigationItem[];
  onScrollToUserMessage?: (patchKey: string) => void;
  getActiveTurnPatchKey?: () => string | null;
  tokenUsageInfo?: ContextUsageInfo | null;
  supportsContextUsage?: boolean;
  dropzone?: DropzoneProps;
  /** Mobile layout active — enlarges approval targets and shows a one-tap Deny. */
  isMobile?: boolean;
  /** Embedded surfaces bind one session and provide their own work selector. */
  showSessionControls?: boolean;
  compact?: boolean;
}

function defaultExecutorLabel(executor: string) {
  return executor
    .replace(/[_-]+/g, ' ')
    .toLowerCase()
    .replace(/\b\w/g, (char) => char.toUpperCase());
}

function defaultFormatSessionDate(createdAt: string | Date) {
  const date = createdAt instanceof Date ? createdAt : new Date(createdAt);
  if (Number.isNaN(date.getTime())) {
    return String(createdAt);
  }

  return date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  });
}

function fallbackDecision(
  action: SessionRuntimeAction,
  allowed: boolean
): SessionRuntimeActionDecision {
  return {
    action,
    allowed,
    reason: allowed ? null : 'unknown_runtime',
  };
}

/**
 * Full-featured chat box for session mode.
 * Supports queue, stop, attach, feedback mode, stats, and session switching.
 */
export function SessionChatBox<TExecutor extends string = string>({
  status,
  editor,
  renderEditor,
  actions,
  actionPolicy,
  getActionPolicy,
  isConfigLoading = false,
  session,
  stats,
  feedbackMode,
  editMode,
  approvalMode,
  askQuestionMode,
  reviewComments,
  toolbarActions,
  modelSelector,
  resumeSelector,
  error,
  repoIds,
  agent,
  executor,
  formatExecutorLabel = defaultExecutorLabel,
  agentBrand,
  emptyExecutorLabel = 'Select Executor',
  renderAgentIcon,
  formatSessionDate = defaultFormatSessionDate,
  todos,
  inProgressTodo,
  localAttachments,
  onPrCommentClick,
  onViewCode,
  onOpenWorkspace,
  onScrollToPreviousMessage,
  userMessageTurns,
  onScrollToUserMessage,
  getActiveTurnPatchKey,
  tokenUsageInfo,
  supportsContextUsage,
  dropzone,
  isMobile,
  showSessionControls = true,
  compact = false,
}: SessionChatBoxProps<TExecutor>) {
  const { t } = useTranslation('tasks');
  const fileInputRef = useRef<HTMLInputElement>(null);
  const askQuestionBannerRef = useRef<AskUserQuestionBannerHandle>(null);
  const toolbarIconButtonClass =
    'min-h-8 min-w-8 shrink-0 rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand focus-visible:ring-inset [@media(pointer:coarse)]:min-h-11 [@media(pointer:coarse)]:min-w-11';

  // Determine if in feedback mode, edit mode, or approval mode
  const isInFeedbackMode = feedbackMode?.isActive ?? false;
  const isInEditMode = editMode?.isActive ?? false;
  const isInApprovalMode = approvalMode?.isActive ?? false;
  const isInAskQuestionMode = askQuestionMode?.isActive ?? false;

  // Key to force editor remount when entering feedback/edit/approval/question mode (triggers auto-focus)
  const focusKey = isInFeedbackMode
    ? 'feedback'
    : isInEditMode
      ? 'edit'
      : isInApprovalMode
        ? 'approval'
        : isInAskQuestionMode
          ? 'question'
          : 'normal';

  // Derived state from status
  const isEditorDisabled = Boolean(
    feedbackMode?.isSubmitting ||
      editMode?.isSubmitting ||
      approvalMode?.isSubmitting ||
      askQuestionMode?.isSubmitting
  );
  const isDisabled = Boolean(
    status === 'sending' || status === 'stopping' || isEditorDisabled
  );
  const hasContent =
    editor.value.trim().length > 0 || (reviewComments?.count ?? 0) > 0;
  const fallbackCanAct =
    hasContent && !['sending', 'stopping', 'queue-loading'].includes(status);
  const sendInitialDecision =
    actionPolicy?.send_initial ??
    fallbackDecision('send_initial', status === 'idle' && fallbackCanAct);
  const sendFollowUpDecision =
    actionPolicy?.send_follow_up ??
    fallbackDecision('send_follow_up', status === 'idle' && fallbackCanAct);
  const queueFollowUpDecision =
    actionPolicy?.queue_follow_up ??
    fallbackDecision('queue_follow_up', status === 'running' && fallbackCanAct);
  const cancelQueueDecision =
    actionPolicy?.cancel_queue ??
    fallbackDecision('cancel_queue', status === 'queued');
  const stopDecision =
    actionPolicy?.stop ??
    fallbackDecision('stop', status === 'running' || status === 'queued');
  const approveDecision =
    actionPolicy?.approve ??
    fallbackDecision(
      'approve',
      isInApprovalMode &&
        !approvalMode?.isTimedOut &&
        !approvalMode?.isSubmitting
    );
  const requestChangesDecision =
    actionPolicy?.request_changes ??
    fallbackDecision(
      'request_changes',
      isInApprovalMode &&
        hasContent &&
        !approvalMode?.isTimedOut &&
        !approvalMode?.isSubmitting
    );
  const answerQuestionDecision =
    actionPolicy?.answer_question ??
    fallbackDecision(
      'answer_question',
      isInAskQuestionMode &&
        !askQuestionMode?.isTimedOut &&
        !askQuestionMode?.isSubmitting
    );
  const isInitialSendMode =
    session.isInitialSendMode ?? session.isNewSessionMode;
  const normalSendDecision =
    isInitialSendMode ||
    (sendInitialDecision.allowed && !sendFollowUpDecision.allowed)
      ? sendInitialDecision
      : sendFollowUpDecision;
  const isQueued = status === 'queued';
  const isRunning = status === 'running' || status === 'queued';
  const activityState =
    isInFeedbackMode || isInEditMode || isInApprovalMode || isInAskQuestionMode
      ? null
      : status === 'sending' || status === 'stopping'
        ? status
        : isRunning
          ? 'running'
          : null;
  const areContentInsertActionsDisabled = isDisabled || isQueued;
  const sendActionLabel = t('conversation.actions.send');
  const queueActionLabel = t('conversation.actions.queue');
  const cancelQueueActionLabel = t('conversation.actions.cancelQueue');
  const stopActionLabel = t('conversation.actions.stop');
  const approveActionLabel = t('conversation.actions.approve');
  const requestChangesActionLabel = t('conversation.actions.requestChanges');
  const denyActionLabel = t('conversation.actions.deny');
  const blockedSendHint =
    !normalSendDecision.allowed &&
    !(isConfigLoading && normalSendDecision.reason === 'no_executor') &&
    !INLINE_BLOCKED_HINT_EXCLUDED_REASONS.has(normalSendDecision.reason ?? '')
      ? getSessionRuntimeActionBlockedReasonLabel(normalSendDecision.reason)
      : null;
  const hasInputTools =
    Boolean(onPrCommentClick) || Boolean(toolbarActions?.items.length);

  const placeholder = isInFeedbackMode
    ? t('conversation.composer.feedbackPlaceholder')
    : isInEditMode
      ? t('conversation.composer.editPlaceholder')
      : isInApprovalMode
        ? t('conversation.composer.changesPlaceholder')
        : isInAskQuestionMode
          ? t('conversation.composer.answerPlaceholder')
          : isInitialSendMode && session.sessions.length === 0
            ? t('conversation.sessions.emptyWorkspacePlaceholder', {
                defaultValue: 'Describe what you want to work on...',
              })
            : t('conversation.composer.messagePlaceholder');

  // Cmd+Enter handler
  const handleCmdEnter = (markdown: string) => {
    const snapshotHasContent =
      markdown.trim().length > 0 || (reviewComments?.count ?? 0) > 0;
    const snapshotPolicy = getActionPolicy?.(markdown);
    const canSubmitSnapshot =
      snapshotHasContent && !isDisabled && status !== 'queue-loading';
    const sendDecision = snapshotPolicy
      ? isInitialSendMode ||
        (snapshotPolicy.send_initial.allowed &&
          !snapshotPolicy.send_follow_up.allowed)
        ? snapshotPolicy.send_initial
        : snapshotPolicy.send_follow_up
      : normalSendDecision;
    const queueDecision =
      snapshotPolicy?.queue_follow_up ?? queueFollowUpDecision;
    // AskUserQuestion mode: Enter submits custom text as answer
    if (
      isInAskQuestionMode &&
      snapshotHasContent &&
      answerQuestionDecision.allowed
    ) {
      askQuestionBannerRef.current?.submitCustomAnswer(markdown);
      editor.onChange('');
      return;
    }
    // Approval mode: Cmd+Enter triggers approve or request changes based on input
    if (isInApprovalMode && !approvalMode?.isTimedOut) {
      if (
        snapshotHasContent &&
        (snapshotPolicy?.request_changes ?? requestChangesDecision).allowed
      ) {
        approvalMode?.onRequestChanges(markdown);
      } else if (approveDecision.allowed) {
        approvalMode?.onApprove();
      }
      return;
    }
    if (isInFeedbackMode && canSubmitSnapshot && !feedbackMode?.isTimedOut) {
      feedbackMode?.onSubmitFeedback(markdown);
    } else if (isInEditMode && canSubmitSnapshot) {
      editMode?.onSubmitEdit(markdown);
    } else if (status === 'running' && queueDecision.allowed) {
      actions.onQueue(markdown);
    } else if (status === 'idle' && sendDecision.allowed) {
      actions.onSend(markdown);
    }
  };

  // File input handlers
  const handleFileInputChange = (e: ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.target.files || []);
    if (files.length > 0) {
      actions.onPasteFiles(files);
    }
    e.target.value = '';
  };

  const handleAttachClick = () => {
    fileInputRef.current?.click();
  };

  const {
    sessions,
    selectedSessionId,
    onSelectSession,
    isNewSessionMode,
    onNewSession,
    onRenameSession,
    onDeleteSession,
    deletingSessionId,
  } = session;
  const hasSessions = sessions.length > 0;
  const showSessionMenu = showSessionControls && hasSessions;
  const selectedSessionObj = sessions.find((s) => s.id === selectedSessionId);

  // Stats
  const filesChanged = stats?.filesChanged ?? 0;
  const linesAdded = stats?.linesAdded;
  const linesRemoved = stats?.linesRemoved;
  const hasDiffStats =
    !stats?.diffStatsUnavailable &&
    (filesChanged > 0 || (linesAdded ?? 0) > 0 || (linesRemoved ?? 0) > 0);

  // Render action buttons based on status
  const renderActionButtons = () => {
    // Feedback mode takes precedence
    if (isInFeedbackMode) {
      if (feedbackMode?.isTimedOut) {
        return (
          <PrimaryButton
            variant="secondary"
            onClick={feedbackMode.onCancel}
            value={t('conversation.actions.cancel')}
          />
        );
      }
      return (
        <>
          <PrimaryButton
            variant="secondary"
            onClick={feedbackMode?.onCancel}
            value={t('conversation.actions.cancel')}
          />
          <PrimaryButton
            onClick={() => handleCmdEnter(editor.getValue?.() ?? editor.value)}
            disabled={!fallbackCanAct || feedbackMode?.isSubmitting}
            actionIcon={feedbackMode?.isSubmitting ? 'spinner' : undefined}
            value={t('conversation.actions.submitFeedback')}
          />
        </>
      );
    }

    // Edit mode
    if (isInEditMode) {
      return (
        <>
          <PrimaryButton
            variant="secondary"
            onClick={editMode?.onCancel}
            value={t('conversation.actions.cancel')}
          />
          <PrimaryButton
            onClick={() => handleCmdEnter(editor.getValue?.() ?? editor.value)}
            disabled={!fallbackCanAct || editMode?.isSubmitting}
            actionIcon={editMode?.isSubmitting ? 'spinner' : undefined}
            value={t('conversation.retry')}
          />
        </>
      );
    }

    // Approval mode
    if (isInApprovalMode) {
      if (approvalMode?.isTimedOut) {
        return (
          <PrimaryButton
            variant="secondary"
            onClick={actions.onStop}
            disabled={!stopDecision.allowed}
            value={stopActionLabel}
            {...getActionButtonA11yProps(stopActionLabel, stopDecision)}
          />
        );
      }

      const hasMessage = editor.value.trim().length > 0;

      // Mobile: surface Stop + Deny + Approve as >=44px targets so a one-tap
      // deny is reachable without first typing a reason. Deny sends the typed
      // text as the reason when present, otherwise a default reason. Desktop
      // keeps the type-to-Request-Changes affordance below.
      if (isMobile) {
        return (
          <>
            <PrimaryButton
              variant="secondary"
              onClick={actions.onStop}
              disabled={!stopDecision.allowed}
              className="min-h-[44px]"
              value={stopActionLabel}
              {...getActionButtonA11yProps(stopActionLabel, stopDecision)}
            />
            <PrimaryButton
              variant="tertiary"
              onClick={approvalMode?.onDeny}
              disabled={!approveDecision.allowed}
              actionIcon={approvalMode?.isSubmitting ? 'spinner' : undefined}
              className="min-h-[44px]"
              value={denyActionLabel}
              {...getActionButtonA11yProps(denyActionLabel, approveDecision)}
            />
            <PrimaryButton
              onClick={approvalMode?.onApprove}
              disabled={!approveDecision.allowed}
              actionIcon={approvalMode?.isSubmitting ? 'spinner' : undefined}
              className="min-h-[44px]"
              value={approveActionLabel}
              {...getActionButtonA11yProps(approveActionLabel, approveDecision)}
            />
          </>
        );
      }

      return (
        <>
          <PrimaryButton
            variant="secondary"
            onClick={actions.onStop}
            disabled={!stopDecision.allowed}
            value={stopActionLabel}
            {...getActionButtonA11yProps(stopActionLabel, stopDecision)}
          />
          {hasMessage ? (
            <PrimaryButton
              onClick={() =>
                handleCmdEnter(editor.getValue?.() ?? editor.value)
              }
              disabled={!requestChangesDecision.allowed}
              actionIcon={approvalMode?.isSubmitting ? 'spinner' : undefined}
              value={requestChangesActionLabel}
              {...getActionButtonA11yProps(
                requestChangesActionLabel,
                requestChangesDecision
              )}
            />
          ) : (
            <PrimaryButton
              onClick={approvalMode?.onApprove}
              disabled={!approveDecision.allowed}
              actionIcon={approvalMode?.isSubmitting ? 'spinner' : undefined}
              value={approveActionLabel}
              {...getActionButtonA11yProps(approveActionLabel, approveDecision)}
            />
          )}
        </>
      );
    }

    // AskUserQuestion mode
    if (isInAskQuestionMode) {
      if (askQuestionMode?.isTimedOut) {
        return (
          <PrimaryButton
            variant="secondary"
            onClick={actions.onStop}
            disabled={!stopDecision.allowed}
            value={stopActionLabel}
            {...getActionButtonA11yProps(stopActionLabel, stopDecision)}
          />
        );
      }

      const hasMessage = editor.value.trim().length > 0;

      return (
        <>
          <PrimaryButton
            variant="secondary"
            onClick={actions.onStop}
            disabled={!stopDecision.allowed}
            value={stopActionLabel}
            {...getActionButtonA11yProps(stopActionLabel, stopDecision)}
          />
          {hasMessage && (
            <PrimaryButton
              onClick={() => {
                askQuestionBannerRef.current?.submitCustomAnswer(
                  editor.getValue?.() ?? editor.value
                );
                editor.onChange('');
              }}
              disabled={!answerQuestionDecision.allowed}
              actionIcon={askQuestionMode?.isSubmitting ? 'spinner' : undefined}
              value={sendActionLabel}
              {...getActionButtonA11yProps(
                sendActionLabel,
                answerQuestionDecision
              )}
            />
          )}
        </>
      );
    }

    switch (status) {
      case 'idle':
        return (
          <PrimaryButton
            onClick={() => handleCmdEnter(editor.getValue?.() ?? editor.value)}
            disabled={!normalSendDecision.allowed}
            value={sendActionLabel}
            {...getActionButtonA11yProps(sendActionLabel, normalSendDecision)}
          />
        );

      case 'sending':
        return (
          <PrimaryButton
            onClick={stopDecision.allowed ? actions.onStop : undefined}
            disabled={!stopDecision.allowed}
            className="disabled:bg-brand disabled:text-on-brand"
            actionIcon={stopDecision.allowed ? SquareIcon : undefined}
            value={stopDecision.allowed ? stopActionLabel : sendActionLabel}
            aria-label={
              stopDecision.allowed
                ? stopActionLabel
                : t('conversation.actions.sending')
            }
          />
        );

      case 'running':
        return (
          <>
            <PrimaryButton
              onClick={() =>
                handleCmdEnter(editor.getValue?.() ?? editor.value)
              }
              disabled={!queueFollowUpDecision.allowed}
              value={queueActionLabel}
              {...getActionButtonA11yProps(
                queueActionLabel,
                queueFollowUpDecision
              )}
            />
            <PrimaryButton
              onClick={actions.onStop}
              disabled={!stopDecision.allowed}
              variant="secondary"
              value={stopActionLabel}
              actionIcon={SquareIcon}
              {...getActionButtonA11yProps(stopActionLabel, stopDecision)}
            />
          </>
        );

      case 'queued':
        return (
          <>
            <PrimaryButton
              onClick={actions.onCancelQueue}
              disabled={!cancelQueueDecision.allowed}
              value={cancelQueueActionLabel}
              actionIcon={XIcon}
              {...getActionButtonA11yProps(
                cancelQueueActionLabel,
                cancelQueueDecision
              )}
            />
            <PrimaryButton
              onClick={actions.onStop}
              disabled={!stopDecision.allowed}
              variant="secondary"
              value={stopActionLabel}
              actionIcon={SquareIcon}
              {...getActionButtonA11yProps(stopActionLabel, stopDecision)}
            />
          </>
        );

      case 'stopping':
        return (
          <PrimaryButton
            disabled
            value={stopActionLabel}
            actionIcon={SquareIcon}
            {...getActionButtonA11yProps(stopActionLabel, stopDecision)}
            aria-label={t('conversation.actions.stopping')}
          />
        );
      case 'queue-loading':
        return (
          <PrimaryButton
            disabled
            value={t('conversation.actions.loading')}
            actionIcon="spinner"
          />
        );
      case 'feedback':
      case 'edit':
        return null;
    }
  };

  // Banner content
  const renderBanner = () => {
    const banners: ReactNode[] = [];

    // Review comments banner
    if (reviewComments && reviewComments.count > 0) {
      banners.push(
        <div
          key="review-comments"
          className="bg-accent/5 border-b px-double py-base flex items-center gap-base"
        >
          <ChatCircleIcon className="h-4 w-4 text-brand flex-shrink-0" />
          <span className="text-sm text-normal flex-1">
            {t('conversation.reviewComments.count', {
              count: reviewComments.count,
            })}
          </span>
          <button
            onClick={reviewComments.onClear}
            className="text-low hover:text-normal transition-colors p-1 -m-1"
            title={t('conversation.actions.clearReviewComments')}
          >
            <TrashIcon className="h-4 w-4" />
          </button>
        </div>
      );
    }

    // AskUserQuestion banner (renders above input)
    if (isInAskQuestionMode && askQuestionMode) {
      banners.push(
        <AskUserQuestionBanner
          key="ask-question"
          ref={askQuestionBannerRef}
          questions={askQuestionMode.questions}
          onSubmitAnswers={askQuestionMode.onSubmitAnswers}
          isSubmitting={askQuestionMode.isSubmitting}
          isTimedOut={askQuestionMode.isTimedOut}
          error={askQuestionMode.error ?? null}
        />
      );
    }

    // Queued message banner
    if (isQueued) {
      banners.push(
        <div
          key="queued"
          className="bg-secondary border-b px-double py-base flex items-center gap-base"
        >
          <ClockIcon className="h-4 w-4 text-low" />
          <span className="text-sm text-low">
            {t('followUp.queuedMessage')}
          </span>
        </div>
      );
    }

    return banners.length > 0 ? <>{banners}</> : null;
  };

  // Combine errors
  const displayError =
    feedbackMode?.error ??
    approvalMode?.error ??
    askQuestionMode?.error ??
    error;

  // Determine visual variant
  const getVisualVariant = () => {
    if (isInFeedbackMode) return VisualVariant.FEEDBACK;
    if (isInEditMode) return VisualVariant.EDIT;
    if (isInApprovalMode || isInAskQuestionMode) return VisualVariant.PLAN;
    return VisualVariant.NORMAL;
  };

  return (
    <ChatBoxBase
      compact={compact}
      editor={renderEditor({
        focusKey,
        placeholder,
        value: editor.value,
        onChange: editor.onChange,
        onCmdEnter: handleCmdEnter,
        // Ordinary sends keep the editor focused and allow a newer draft.
        // Submission policy and attachment actions still guard pending work.
        disabled: isEditorDisabled,
        repoIds,
        executor: agent || executor?.selected || null,
        onPasteFiles: (files) => {
          if (!areContentInsertActionsDisabled) actions.onPasteFiles(files);
        },
        localAttachments,
      })}
      error={displayError}
      banner={renderBanner()}
      visualVariant={getVisualVariant()}
      dropzone={dropzone}
      modelSelector={modelSelector}
      headerLeft={
        <>
          {/* New session mode: agent icon + executor dropdown */}
          {isNewSessionMode && executor && (
            <>
              <span
                data-agent-brand={agentBrand}
                className="flex min-w-0 items-center gap-2"
              >
                {renderAgentIcon?.(agent, 'size-icon-xl')}
                <ToolbarDropdown
                  className="vk-agent-brand-label"
                  label={
                    executor.selected
                      ? formatExecutorLabel(executor.selected)
                      : emptyExecutorLabel
                  }
                >
                  <DropdownMenuLabel>
                    {t('conversation.executors')}
                  </DropdownMenuLabel>
                  {executor.options.map((exec) => (
                    <DropdownMenuItem
                      key={exec}
                      icon={executor.selected === exec ? CheckIcon : undefined}
                      onClick={() => executor.onChange(exec)}
                    >
                      {formatExecutorLabel(exec)}
                    </DropdownMenuItem>
                  ))}
                </ToolbarDropdown>
              </span>
              {executor.afterSelector}
            </>
          )}
          {!isNewSessionMode && agent && (
            <span
              data-agent-brand={agentBrand}
              className="vk-agent-brand-label flex min-w-0 items-center gap-1.5"
              title={formatExecutorLabel(agent)}
            >
              {renderAgentIcon?.(agent, 'size-icon-xl')}
              <span className="max-w-[10rem] truncate text-sm">
                {formatExecutorLabel(agent)}
              </span>
            </span>
          )}
        </>
      }
      status={
        <>
          {activityState && (
            <ChatComposerActivity
              state={activityState}
              label={
                activityState === 'running'
                  ? t('conversation.inProgress')
                  : t(`conversation.actions.${activityState}`)
              }
              detail={isRunning ? inProgressTodo?.content : undefined}
            />
          )}
          {/* Existing session mode: show in-progress todo when running, otherwise file stats */}
          {!isInitialSendMode && (
            <>
              {!(isRunning && inProgressTodo) && (
                <>
                  {stats?.hasConflicts && (
                    <button
                      type="button"
                      className="flex items-center gap-1 text-warning text-sm min-w-0 cursor-pointer hover:underline"
                      title={t('conversation.approval.conflictWarning')}
                      onClick={stats.onResolveConflicts}
                    >
                      <WarningIcon className="size-icon-sm flex-shrink-0" />
                      <span className="truncate">
                        {t('conversation.approval.conflicts', {
                          count: stats.conflictedFilesCount,
                        })}
                      </span>
                    </button>
                  )}
                  {hasDiffStats && onOpenWorkspace ? (
                    <PrimaryButton
                      variant="secondary"
                      onClick={onOpenWorkspace}
                      value="Open Workspace"
                      actionIcon={ArrowsOutIcon}
                      className="min-w-0"
                    />
                  ) : hasDiffStats && onViewCode ? (
                    <PrimaryButton
                      variant="tertiary"
                      onClick={onViewCode}
                      className="min-w-0"
                    >
                      <span className="text-sm space-x-half whitespace-nowrap truncate">
                        <span>
                          {t('diff.filesChanged', { count: filesChanged })}
                        </span>
                        {(linesAdded !== undefined ||
                          linesRemoved !== undefined) && (
                          <span className="space-x-half">
                            {linesAdded !== undefined && (
                              <span className="text-success">
                                +{linesAdded}
                              </span>
                            )}
                            {linesRemoved !== undefined && (
                              <span className="text-error">
                                -{linesRemoved}
                              </span>
                            )}
                          </span>
                        )}
                      </span>
                    </PrimaryButton>
                  ) : stats?.diffStatsUnavailable ? (
                    <span className="text-sm text-low min-w-0 truncate">
                      {t('diff.unavailable', {
                        defaultValue: 'Changes unavailable',
                      })}
                    </span>
                  ) : hasDiffStats ? (
                    <span className="text-sm text-low space-x-half whitespace-nowrap truncate min-w-0">
                      <span>
                        {t('diff.filesChanged', { count: filesChanged })}
                      </span>
                      {(linesAdded !== undefined ||
                        linesRemoved !== undefined) && (
                        <span className="space-x-half">
                          {linesAdded !== undefined && (
                            <span className="text-success">+{linesAdded}</span>
                          )}
                          {linesRemoved !== undefined && (
                            <span className="text-error">-{linesRemoved}</span>
                          )}
                        </span>
                      )}
                    </span>
                  ) : null}
                </>
              )}
            </>
          )}
        </>
      }
      footerLeft={
        <>
          <ToolbarIconButton
            type="button"
            icon={PaperclipIcon}
            className={cn(
              toolbarIconButtonClass,
              status === 'sending' && !isEditorDisabled && '!opacity-100'
            )}
            aria-label={t('tasks:taskFormDialog.attachFile')}
            title={t('tasks:taskFormDialog.attachFile')}
            onClick={handleAttachClick}
            disabled={areContentInsertActionsDisabled}
          />
          <input
            ref={fileInputRef}
            type="file"
            multiple
            className="hidden"
            onChange={handleFileInputChange}
          />
        </>
      }
      headerRight={
        <>
          {resumeSelector}
          {/* Navigation and session management stay away from model controls. */}
          {!isInitialSendMode && (
            <>
              {onScrollToPreviousMessage && (
                <TurnNavigationPopup
                  turns={userMessageTurns ?? []}
                  onNavigateToTurn={onScrollToUserMessage ?? (() => {})}
                  getActiveTurnPatchKey={getActiveTurnPatchKey}
                >
                  <ToolbarIconButton
                    type="button"
                    icon={ClockCounterClockwiseIcon}
                    className={toolbarIconButtonClass}
                    title={t('conversation.actions.scrollToPreviousMessage')}
                    aria-label={t(
                      'conversation.actions.scrollToPreviousMessage'
                    )}
                    onClick={onScrollToPreviousMessage}
                  />
                </TurnNavigationPopup>
              )}
            </>
          )}
          {(showSessionMenu || hasInputTools) && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className={`flex items-center justify-center text-low hover:text-normal ${toolbarIconButtonClass}`}
                  aria-label={t('conversation.composer.moreActions')}
                  title={t('conversation.composer.moreActions')}
                  data-conversation-actions-trigger
                  disabled={
                    isInFeedbackMode || isInEditMode || isInApprovalMode
                  }
                >
                  <DotsThreeIcon className="size-icon-base" weight="bold" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" side="top">
                {showSessionMenu && (
                  <>
                    {/* New Session option */}
                    <DropdownMenuItem
                      icon={isInitialSendMode ? CheckIcon : PlusIcon}
                      onClick={() => onNewSession?.()}
                    >
                      {t('conversation.sessions.newSession')}
                    </DropdownMenuItem>
                    {sessions.length > 0 && <DropdownMenuSeparator />}
                    {sessions.length > 0 ? (
                      <>
                        <DropdownMenuLabel>
                          {t('conversation.sessions.label')}
                        </DropdownMenuLabel>
                        {sessions.map((s, index) => (
                          <DropdownMenuItem
                            key={s.id}
                            icon={
                              !isInitialSendMode && s.id === selectedSessionId
                                ? CheckIcon
                                : undefined
                            }
                            onClick={() => onSelectSession(s.id)}
                          >
                            <span className="flex items-center gap-1.5 max-w-[200px]">
                              {renderAgentIcon?.(
                                s.executor ?? null,
                                'size-icon-base shrink-0'
                              )}
                              <span className="truncate">
                                {s.name
                                  ? s.name
                                  : index === 0
                                    ? t('conversation.sessions.latest')
                                    : formatSessionDate(s.created_at)}
                              </span>
                            </span>
                          </DropdownMenuItem>
                        ))}
                      </>
                    ) : (
                      <DropdownMenuItem disabled>
                        {t('conversation.sessions.noPreviousSessions')}
                      </DropdownMenuItem>
                    )}
                    {onRenameSession &&
                      selectedSessionId &&
                      !isInitialSendMode && (
                        <>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            icon={PencilSimpleIcon}
                            onClick={() =>
                              onRenameSession(
                                selectedSessionId,
                                selectedSessionObj?.name ?? ''
                              )
                            }
                          >
                            {t('conversation.sessions.rename')}
                          </DropdownMenuItem>
                        </>
                      )}
                    {onDeleteSession &&
                      selectedSessionId &&
                      !isInitialSendMode && (
                        <DropdownMenuItem
                          icon={TrashIcon}
                          variant="destructive"
                          disabled={
                            isRunning ||
                            isDisabled ||
                            deletingSessionId === selectedSessionId
                          }
                          onClick={() =>
                            onDeleteSession(
                              selectedSessionId,
                              selectedSessionObj?.name ?? ''
                            )
                          }
                        >
                          {t('conversation.sessions.delete', 'Delete session')}
                        </DropdownMenuItem>
                      )}
                  </>
                )}
                {showSessionMenu && hasInputTools && <DropdownMenuSeparator />}
                {onPrCommentClick && (
                  <DropdownMenuItem
                    icon={GithubLogoIcon}
                    onClick={onPrCommentClick}
                    disabled={areContentInsertActionsDisabled}
                  >
                    {t('conversation.composer.addPrComments')}
                  </DropdownMenuItem>
                )}
                {onPrCommentClick && Boolean(toolbarActions?.items.length) && (
                  <DropdownMenuSeparator />
                )}
                {toolbarActions?.items.map((item) => (
                  <DropdownMenuItem
                    key={item.id}
                    icon={item.icon}
                    onClick={item.onClick}
                    disabled={
                      Boolean(item.disabled) ||
                      (item.requiresIdle !== false && (isDisabled || isRunning))
                    }
                  >
                    {item.label}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
          {Boolean(todos?.length) && (
            <TodoProgressPopup
              todos={todos ?? []}
              className={toolbarIconButtonClass}
            />
          )}
          {supportsContextUsage &&
            tokenUsageInfo &&
            tokenUsageInfo.model_context_window > 0 && (
              <ContextUsageGauge
                tokenUsageInfo={tokenUsageInfo}
                className="min-h-8 min-w-8"
              />
            )}
        </>
      }
      hint={
        blockedSendHint && (
          <span
            className="flex min-w-0 items-center gap-half text-xs text-low"
            title={blockedSendHint}
          >
            <WarningIcon className="size-icon-xs flex-shrink-0" />
            <span className="truncate">{blockedSendHint}</span>
          </span>
        )
      }
      footerRight={
        <div className="flex min-h-8 w-[10.5rem] max-w-full flex-wrap items-center justify-end gap-half [&_button]:min-w-20 [&_button]:justify-center">
          {renderActionButtons()}
        </div>
      }
    />
  );
}
