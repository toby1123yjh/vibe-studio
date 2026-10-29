import { useMemo, useCallback, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import {
  ActionType,
  BaseAgentCapability,
  ExecutionProcessStatus,
  NormalizedEntry,
  ToolStatus,
  ToolResult,
  TodoItem,
  type ExecutionProcess,
  type RepoWithTargetBranch,
} from 'shared/types';
import type { WorkspaceWithSession } from '@/shared/types/attempt';
import { parseDiffStats } from '@/shared/lib/diffStatsParser';
import {
  usePersistedExpanded,
  type PersistKey,
} from '@/shared/stores/useUiPreferencesStore';
import { getActualTheme } from '@/shared/lib/theme';
import { getFileIcon } from '@/shared/lib/fileTypeIcon';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { useTheme } from '@/shared/hooks/useTheme';
import WYSIWYGEditor from '@/shared/components/WYSIWYGEditor';
import { useMessageEditContext } from '../model/contexts/MessageEditContext';
import type { UseResetProcessResult } from '../model/hooks/useResetProcess';
import { useChangesViewActions } from '@/shared/hooks/useChangesView';
import { useLogsPanelActions } from '@/shared/hooks/useLogsPanel';
import {
  useWorkspaceFilePreviewActions,
  useWorkspaceFilePreviewResolver,
} from '@/features/workspace-files';
import { cn } from '@/shared/lib/utils';
import {
  ScriptFixerDialog,
  type ScriptType,
} from '@/shared/dialogs/scripts/ScriptFixerDialog';
import { ChatToolSummary } from '@vibe/ui/components/ChatToolSummary';
import { ChatTodoList } from '@vibe/ui/components/ChatTodoList';
import {
  ChatFileEntry,
  type ChatFileEntryDiffInput,
} from '@vibe/ui/components/ChatFileEntry';
import { ChatApprovalCard } from '@vibe/ui/components/ChatApprovalCard';
import { ChatUserMessage } from '@vibe/ui/components/ChatUserMessage';
import { ChatAssistantMessage } from '@vibe/ui/components/ChatAssistantMessage';
import { ChatSystemMessage } from '@vibe/ui/components/ChatSystemMessage';
import { ChatThinkingMessage } from '@vibe/ui/components/ChatThinkingMessage';
import { ChatErrorMessage } from '@vibe/ui/components/ChatErrorMessage';
import { ChatScriptEntry } from '@vibe/ui/components/ChatScriptEntry';
import { ChatSubagentEntry } from '@vibe/ui/components/ChatSubagentEntry';
import { ChatAggregatedToolEntries } from '@vibe/ui/components/ChatAggregatedToolEntries';
import { ChatAggregatedDiffEntries } from '@vibe/ui/components/ChatAggregatedDiffEntries';
import { ChatAggregatedFileChangeEntries } from '@vibe/ui/components/ChatAggregatedFileChangeEntries';
import { ChatCollapsedThinking } from '@vibe/ui/components/ChatCollapsedThinking';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import { AppChatMarkdown } from './AppChatMarkdown';
import {
  DiffViewBody,
  useDiffData,
} from '@vibe/ui/components/PierreConversationDiff';
import { inIframe, openFileInVSCode } from '@/integrations/vscode/bridge';
import { useDiffViewMode } from '@/shared/stores/useDiffViewStore';
import { useExecutionProcessesContext } from '@/shared/hooks/useExecutionProcessesContext';
import type {
  AggregatedPatchGroup,
  AggregatedDiffGroup,
  AggregatedFileChangeGroup,
  AggregatedThinkingGroup,
  PatchTypeWithKey,
} from '@/shared/hooks/useConversationHistory/types';
import {
  CaretDownIcon,
  FileTextIcon,
  ListMagnifyingGlassIcon,
  GlobeIcon,
  PencilSimpleIcon,
  TerminalWindowIcon,
} from '@phosphor-icons/react';

type Props = {
  expansionKey: string;
  executionProcessId?: string;
  canonicalActive?: boolean;
  workspaceWithSession: WorkspaceWithSession;
  resetAction: UseResetProcessResult;
  readOnly?: boolean;
  repos: RepoWithTargetBranch[];
  entry: NormalizedEntry | null;
  aggregatedGroup: AggregatedPatchGroup | null;
  aggregatedDiffGroup: AggregatedDiffGroup | null;
  aggregatedFileChangeGroup: AggregatedFileChangeGroup | null;
  aggregatedThinkingGroup: AggregatedThinkingGroup | null;
};

type FileEditAction = Extract<ActionType, { action: 'file_edit' }>;

function isActiveToolStatus(
  status: ToolStatus,
  executionProcess: ExecutionProcess | null,
  canonicalActive?: boolean
) {
  if (canonicalActive !== undefined) return canonicalActive;
  if (status.status === 'pending_approval') return true;
  if (status.status !== 'created') return false;
  return isProcessRunning(executionProcess);
}

function getPatchEntryTimestamp(entry: PatchTypeWithKey) {
  if (entry.type !== 'NORMALIZED_ENTRY') return null;
  return entry.content.timestamp;
}

function getFirstPatchEntryTimestamp(entries: PatchTypeWithKey[]) {
  for (const entry of entries) {
    const timestamp = getPatchEntryTimestamp(entry);
    if (timestamp) return timestamp;
  }
  return null;
}

function getLastPatchEntryTimestamp(entries: PatchTypeWithKey[]) {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const timestamp = getPatchEntryTimestamp(entries[index]);
    if (timestamp) return timestamp;
  }
  return null;
}

function getProcessStartedAt(executionProcess: ExecutionProcess | null) {
  return executionProcess?.started_at ?? executionProcess?.created_at ?? null;
}

function getProcessEndedAt(executionProcess: ExecutionProcess | null) {
  return executionProcess?.completed_at ?? null;
}

function isProcessRunning(executionProcess: ExecutionProcess | null) {
  return executionProcess?.status === ExecutionProcessStatus.running;
}

function getPatchEntryTiming({
  entry,
  isRunning,
  executionProcess,
}: {
  entry: PatchTypeWithKey;
  isRunning: boolean;
  executionProcess: ExecutionProcess | null;
}) {
  const timestamp = getPatchEntryTimestamp(entry);

  return {
    startedAt: timestamp ?? getProcessStartedAt(executionProcess),
    endedAt: isRunning
      ? null
      : (timestamp ?? getProcessEndedAt(executionProcess)),
  };
}

function getPatchGroupTiming({
  entries,
  isRunning,
  executionProcess,
}: {
  entries: PatchTypeWithKey[];
  isRunning: boolean;
  executionProcess: ExecutionProcess | null;
}) {
  return {
    startedAt:
      getFirstPatchEntryTimestamp(entries) ??
      getProcessStartedAt(executionProcess),
    endedAt: isRunning
      ? null
      : (getLastPatchEntryTimestamp(entries) ??
        getProcessEndedAt(executionProcess)),
  };
}

/**
 * Generate tool summary text from action type
 */
function getToolSummary(
  entryType: Extract<NormalizedEntry['entry_type'], { type: 'tool_use' }>,
  t: TFunction<'common'>
): string {
  const { action_type, tool_name } = entryType;

  switch (action_type.action) {
    case 'file_read':
      return t('conversation.toolSummary.read', {
        path: formatPathForDisplay(action_type.path),
      });
    case 'search':
      return t('conversation.toolSummary.searched', {
        query: action_type.query,
      });
    case 'web_fetch':
      return t('conversation.toolSummary.fetched', { url: action_type.url });
    case 'command_run':
      return action_type.command || t('conversation.toolSummary.ranCommand');
    case 'task_create':
      return t('conversation.toolSummary.createdTask', {
        description: action_type.description,
      });
    case 'todo_management':
      return t('conversation.toolSummary.todoOperation', {
        operation: action_type.operation,
      });
    case 'tool':
      return tool_name || t('conversation.tool');
    default:
      return tool_name || t('conversation.tool');
  }
}

/**
 * Extract the actual tool output from action_type.result
 * The output location depends on the action type:
 * - command_run: result.output
 * - tool: result.value (JSON stringified if object)
 * - others: fall back to entry.content
 */
function getToolOutput(
  entryType: Extract<NormalizedEntry['entry_type'], { type: 'tool_use' }>,
  entryContent: string
): string {
  const { action_type } = entryType;

  switch (action_type.action) {
    case 'command_run':
      return action_type.result?.output ?? entryContent;
    case 'tool':
      if (action_type.result?.value != null) {
        return typeof action_type.result.value === 'string'
          ? action_type.result.value
          : JSON.stringify(action_type.result.value, null, 2);
      }
      return entryContent;
    default:
      return entryContent;
  }
}

/**
 * Extract the command from action_type for command_run actions
 */
function getToolCommand(
  entryType: Extract<NormalizedEntry['entry_type'], { type: 'tool_use' }>
): string | undefined {
  const { action_type } = entryType;

  if (action_type.action === 'command_run') {
    return action_type.command;
  }
  return undefined;
}

/**
 * Render tool_use entry types with appropriate components
 */
function renderToolUseEntry(
  entryType: Extract<NormalizedEntry['entry_type'], { type: 'tool_use' }>,
  entry: NormalizedEntry,
  props: Props,
  t: TFunction<'common'>,
  executionProcess: ExecutionProcess | null
): React.ReactNode {
  const { expansionKey, executionProcessId, workspaceWithSession, repos } =
    props;
  const sessionId = workspaceWithSession?.session?.id;
  const { action_type, status } = entryType;
  const active = isActiveToolStatus(
    status,
    executionProcess,
    props.canonicalActive
  );

  // File edit - use ChatFileEntry
  if (action_type.action === 'file_edit') {
    const fileEditAction = action_type as FileEditAction;
    return (
      <>
        {fileEditAction.changes.map((change, idx) => (
          <FileEditEntry
            key={idx}
            path={fileEditAction.path}
            change={change}
            expansionKey={`edit:${expansionKey}:${idx}`}
            status={status}
            active={active}
            workspaceId={workspaceWithSession?.id}
            sessionId={sessionId}
            repos={repos}
          />
        ))}
      </>
    );
  }

  // Plan presentation - use ChatApprovalCard
  if (action_type.action === 'plan_presentation') {
    // Codex can emit an initial empty plan placeholder before content arrives.
    // Suppress empty cards to avoid duplicate-looking plan boxes.
    if (!action_type.plan.trim()) {
      return null;
    }
    return (
      <PlanEntry
        plan={action_type.plan}
        expansionKey={expansionKey}
        workspaceId={workspaceWithSession?.id}
        sessionId={sessionId}
        status={status}
      />
    );
  }

  // Todo management - use ChatTodoList
  if (action_type.action === 'todo_management') {
    return (
      <TodoManagementEntry
        todos={action_type.todos}
        expansionKey={expansionKey}
      />
    );
  }

  // Task/Subagent - use ChatSubagentEntry
  if (action_type.action === 'task_create') {
    return (
      <SubagentEntry
        description={action_type.description}
        subagentType={action_type.subagent_type}
        result={action_type.result}
        expansionKey={expansionKey}
        status={status}
        active={active}
        workspaceId={workspaceWithSession?.id}
        sessionId={sessionId}
      />
    );
  }

  // Script entries (Setup Script, Cleanup Script, Archive Script, Tool Install Script)
  const scriptToolNames = [
    'Setup Script',
    'Cleanup Script',
    'Archive Script',
    'Tool Install Script',
  ];
  if (
    action_type.action === 'command_run' &&
    scriptToolNames.includes(entryType.tool_name)
  ) {
    const exitCode =
      action_type.result?.exit_status?.type === 'exit_code'
        ? action_type.result.exit_status.code
        : null;

    return (
      <ScriptEntryWithFix
        title={entryType.tool_name}
        command={action_type.command}
        processId={executionProcessId ?? ''}
        exitCode={exitCode}
        status={status}
        active={active}
        workspaceId={workspaceWithSession?.id}
        sessionId={sessionId}
        repos={repos}
        readOnly={props.readOnly ?? false}
      />
    );
  }

  // Generic tool pending approval - use plan-style card
  if (status.status === 'pending_approval') {
    // Question approvals are rendered via askQuestionMode in SessionChatBoxContainer.
    // Avoid showing a duplicate inline approval card in the conversation timeline.
    if (action_type.action === 'ask_user_question') {
      return null;
    }
    return (
      <GenericToolApprovalEntry
        toolName={entryType.tool_name}
        content={entry.content}
        expansionKey={expansionKey}
        workspaceId={workspaceWithSession?.id}
        sessionId={sessionId}
        status={status}
      />
    );
  }

  // Other tool uses - use ChatToolSummary
  return (
    <ToolSummaryEntry
      summary={getToolSummary(entryType, t)}
      expansionKey={expansionKey}
      status={status}
      content={getToolOutput(entryType, entry.content)}
      toolName={entryType.tool_name}
      command={getToolCommand(entryType)}
      actionType={action_type.action}
      startedAt={entry.timestamp ?? getProcessStartedAt(executionProcess)}
      endedAt={
        active ? null : (entry.timestamp ?? getProcessEndedAt(executionProcess))
      }
      active={active}
    />
  );
}

function DisplayConversationEntry(props: Props) {
  const { t } = useTranslation('common');
  const { capabilities } = useUserSystem();
  const { executionProcessesByIdAll, executionProcessesByIdVisible } =
    useExecutionProcessesContext();
  const {
    entry,
    aggregatedGroup,
    aggregatedDiffGroup,
    aggregatedFileChangeGroup,
    aggregatedThinkingGroup,
    expansionKey,
    executionProcessId,
    workspaceWithSession,
    resetAction,
  } = props;
  const executionProcess = executionProcessId
    ? (executionProcessesByIdVisible[executionProcessId] ??
      executionProcessesByIdAll[executionProcessId] ??
      null)
    : null;
  const sessionId = workspaceWithSession?.session?.id;
  const executorCanFork = !!(
    workspaceWithSession?.session?.executor &&
    capabilities?.[workspaceWithSession.session.executor]?.includes(
      BaseAgentCapability.SESSION_FORK
    )
  );

  // Handle aggregated groups (consecutive file_read or search entries)
  if (aggregatedGroup) {
    return (
      <AggregatedGroupEntry
        group={aggregatedGroup}
        executionProcess={executionProcess}
      />
    );
  }

  // Handle aggregated diff groups (consecutive file_edit entries for same file)
  if (aggregatedDiffGroup) {
    return (
      <AggregatedDiffGroupEntry
        group={aggregatedDiffGroup}
        executionProcess={executionProcess}
        workspaceId={workspaceWithSession?.id}
        sessionId={sessionId}
        repos={props.repos}
      />
    );
  }

  // Handle aggregated file-change groups (consecutive file_edit entries)
  if (aggregatedFileChangeGroup) {
    return (
      <AggregatedFileChangeGroupEntry
        group={aggregatedFileChangeGroup}
        executionProcess={executionProcess}
        workspaceId={workspaceWithSession?.id}
        sessionId={sessionId}
        repos={props.repos}
      />
    );
  }

  // Handle aggregated thinking groups (thinking entries in previous turns)
  if (aggregatedThinkingGroup) {
    return (
      <AggregatedThinkingGroupEntry
        group={aggregatedThinkingGroup}
        executionProcess={executionProcess}
        workspaceId={workspaceWithSession?.id}
        sessionId={sessionId}
      />
    );
  }

  // If no entry, return null (shouldn't happen in normal usage)
  if (!entry) {
    return null;
  }

  const entryType = entry.entry_type;

  switch (entryType.type) {
    case 'tool_use':
      return renderToolUseEntry(entryType, entry, props, t, executionProcess);

    case 'user_message':
      return (
        <UserMessageEntry
          content={entry.content}
          expansionKey={expansionKey}
          workspaceId={workspaceWithSession?.id}
          sessionId={sessionId}
          executionProcessId={executionProcessId}
          executorCanFork={executorCanFork}
          resetAction={resetAction}
          readOnly={props.readOnly ?? false}
        />
      );

    case 'assistant_message':
      return (
        <AssistantMessageEntry
          content={entry.content}
          workspaceId={workspaceWithSession?.id}
          sessionId={sessionId}
        />
      );

    case 'system_message':
      return (
        <SystemMessageEntry
          content={entry.content}
          expansionKey={expansionKey}
        />
      );

    case 'thinking':
      return (
        <ChatThinkingMessage
          content={entry.content}
          workspaceId={workspaceWithSession?.id}
          startedAt={entry.timestamp ?? getProcessStartedAt(executionProcess)}
          endedAt={executionProcess?.completed_at ?? null}
          active={isProcessRunning(executionProcess)}
          renderMarkdown={({ content, workspaceId, className }) => (
            <AppChatMarkdown
              content={content}
              workspaceId={workspaceId}
              sessionId={sessionId}
              className={className}
              maxWidth={undefined}
            />
          )}
        />
      );

    case 'error_message':
      return (
        <ErrorMessageEntry
          content={entry.content}
          expansionKey={expansionKey}
        />
      );

    case 'next_action':
      // The new design doesn't need the next action bar
      return null;

    case 'token_usage_info':
      // Displayed in the chat header as the context-usage gauge
      return null;

    case 'user_feedback':
      return (
        <UserFeedbackEntry
          content={entry.content}
          deniedTool={entryType.denied_tool}
          workspaceId={workspaceWithSession?.id}
          sessionId={sessionId}
        />
      );

    case 'user_answered_questions':
      return (
        <UserAnsweredQuestionsEntry
          answers={entryType.answers}
          expansionKey={expansionKey}
        />
      );

    case 'loading':
      return <LoadingEntry />;

    default: {
      // Exhaustive check - TypeScript will error if a case is missing
      const _exhaustiveCheck: never = entryType;
      return _exhaustiveCheck;
    }
  }
}

/**
 * File edit entry with expandable diff
 */
function FileEntryDiffBody({
  diffContent,
}: {
  diffContent: ChatFileEntryDiffInput;
}) {
  const { theme } = useTheme();
  const actualTheme = getActualTheme(theme);
  const diffMode = useDiffViewMode();
  const diffData = useDiffData(diffContent);

  if (!diffData.isValid) {
    return null;
  }

  return (
    <DiffViewBody
      fileDiffMetadata={diffData.fileDiffMetadata}
      unifiedDiff={diffData.unifiedDiff}
      isValid={diffData.isValid}
      hideLineNumbers={diffData.hideLineNumbers}
      theme={actualTheme}
      diffMode={diffMode}
    />
  );
}

function FileEditEntry({
  path,
  change,
  expansionKey,
  status,
  active,
  workspaceId,
  sessionId,
  repos,
}: {
  path: string;
  change: FileEditAction['changes'][number];
  expansionKey: string;
  status: ToolStatus;
  active: boolean;
  workspaceId: string | undefined;
  sessionId: string | undefined;
  repos: RepoWithTargetBranch[];
}) {
  // Auto-expand when pending approval
  const pendingApproval = status.status === 'pending_approval';
  const [expanded, toggle] = usePersistedExpanded(
    expansionKey as PersistKey,
    pendingApproval
  );
  const { theme } = useTheme();
  const actualTheme = getActualTheme(theme);
  const { viewFileInChanges, hasDiffPath } = useChangesViewActions();
  const { canOpenWorkspaceFilePreview, openWorkspaceFilePreview } =
    useWorkspaceFilePreviewActions();
  const resolveFilePreviewTarget = useWorkspaceFilePreviewResolver(
    workspaceId,
    repos,
    sessionId
  );
  const FileIcon = useMemo(
    () => getFileIcon(path, actualTheme),
    [path, actualTheme]
  );
  const isVSCode = inIframe();

  // Calculate diff stats for edit changes
  const { additions, deletions } = useMemo(() => {
    if (change.action === 'edit' && change.unified_diff) {
      return parseDiffStats(change.unified_diff);
    }
    return { additions: undefined, deletions: undefined };
  }, [change]);

  // For write actions, count as all additions
  const writeAdditions =
    change.action === 'write' ? change.content.split('\n').length : undefined;

  // Build diff content for rendering when expanded
  const diffContent: ChatFileEntryDiffInput | undefined = useMemo(() => {
    if (change.action === 'edit' && change.unified_diff) {
      return {
        type: 'unified',
        path,
        unifiedDiff: change.unified_diff,
        hasLineNumbers: change.has_line_numbers ?? true,
      };
    }
    // For write actions, use content-based diff (empty old, new content)
    if (change.action === 'write' && change.content) {
      return {
        type: 'content',
        oldContent: '',
        newContent: change.content,
        newPath: path,
      };
    }
    return undefined;
  }, [change, path]);
  const diffPreviewData = useDiffData(
    diffContent ?? { type: 'unified', path, unifiedDiff: '' }
  );
  const hasDiffContent = Boolean(diffContent && diffPreviewData.isValid);

  // Only show "open in changes" button if the file exists in current diffs
  const handleOpenInChanges = useCallback(() => {
    if (!hasDiffPath(path)) return;
    viewFileInChanges(path);
  }, [viewFileInChanges, hasDiffPath, path]);
  const previewResolution = useMemo(
    () =>
      resolveFilePreviewTarget({
        path,
        source: 'chat',
      }),
    [path, resolveFilePreviewTarget]
  );
  const handleOpenFilePreview = useCallback(() => {
    if (
      !canOpenWorkspaceFilePreview ||
      previewResolution.status !== 'resolved'
    ) {
      return;
    }
    openWorkspaceFilePreview(previewResolution.target);
  }, [
    canOpenWorkspaceFilePreview,
    openWorkspaceFilePreview,
    previewResolution,
  ]);
  const handleOpenInVSCode = useCallback((filename: string) => {
    openFileInVSCode(filename, { openAsDiff: false });
  }, []);

  return (
    <ChatFileEntry
      filename={path}
      additions={additions ?? writeAdditions}
      deletions={deletions}
      expanded={expanded}
      onToggle={toggle}
      status={status}
      active={active}
      fileIcon={FileIcon}
      isVSCode={isVSCode}
      onOpenInVSCode={handleOpenInVSCode}
      diffContent={hasDiffContent ? diffContent : undefined}
      renderDiffBody={
        hasDiffContent
          ? (entryDiffContent) => (
              <FileEntryDiffBody diffContent={entryDiffContent} />
            )
          : undefined
      }
      onOpenFilePreview={
        canOpenWorkspaceFilePreview && previewResolution.status === 'resolved'
          ? handleOpenFilePreview
          : undefined
      }
      onOpenInChanges={handleOpenInChanges}
    />
  );
}

/**
 * Plan entry with expandable content
 */
function PlanEntry({
  plan,
  expansionKey,
  workspaceId,
  sessionId,
  status,
}: {
  plan: string;
  expansionKey: string;
  workspaceId: string | undefined;
  sessionId: string | undefined;
  status: ToolStatus;
}) {
  const { t } = useTranslation('common');
  // Expand plans by default when pending approval
  const pendingApproval = status.status === 'pending_approval';
  const [expanded, toggle] = usePersistedExpanded(
    `plan:${expansionKey}`,
    pendingApproval
  );

  // Extract title from plan content (first line or default)
  const title = useMemo(() => {
    const firstLine = plan.split('\n')[0];
    // Remove markdown heading markers
    const cleanTitle = firstLine.replace(/^#+\s*/, '').trim();
    return cleanTitle || t('conversation.plan');
  }, [plan, t]);

  return (
    <ChatApprovalCard
      title={title}
      content={plan}
      expanded={expanded}
      onToggle={toggle}
      workspaceId={workspaceId}
      status={status}
      renderMarkdown={({ content, workspaceId }) => (
        <AppChatMarkdown
          content={content}
          workspaceId={workspaceId}
          sessionId={sessionId}
          className={undefined}
          maxWidth={undefined}
        />
      )}
    />
  );
}

/**
 * Generic tool approval entry - renders with plan-style card when pending approval
 */
function GenericToolApprovalEntry({
  toolName,
  content,
  expansionKey,
  workspaceId,
  sessionId,
  status,
}: {
  toolName: string;
  content: string;
  expansionKey: string;
  workspaceId: string | undefined;
  sessionId: string | undefined;
  status: ToolStatus;
}) {
  const [expanded, toggle] = usePersistedExpanded(
    `tool:${expansionKey}`,
    true // auto-expand for pending approval
  );

  return (
    <ChatApprovalCard
      title={toolName}
      content={content}
      expanded={expanded}
      onToggle={toggle}
      workspaceId={workspaceId}
      status={status}
      renderMarkdown={({ content, workspaceId }) => (
        <AppChatMarkdown
          content={content}
          workspaceId={workspaceId}
          sessionId={sessionId}
          className={undefined}
          maxWidth={undefined}
        />
      )}
    />
  );
}

/**
 * User message entry with expandable content
 */
function UserMessageEntry({
  content,
  expansionKey,
  workspaceId,
  sessionId,
  executionProcessId,
  executorCanFork,
  resetAction,
  readOnly,
}: {
  content: string;
  expansionKey: string;
  workspaceId: string | undefined;
  sessionId: string | undefined;
  executionProcessId: string | undefined;
  executorCanFork: boolean;
  resetAction: UseResetProcessResult;
  readOnly: boolean;
}) {
  const [expanded, toggle] = usePersistedExpanded(`user:${expansionKey}`, true);
  const { startEdit, isEntryGreyed, isInEditMode } = useMessageEditContext();
  const { resetProcess, canResetProcess, isResetPending } = resetAction;

  const isGreyed = isEntryGreyed(expansionKey);

  const handleEdit = () => {
    if (!readOnly && executionProcessId) {
      startEdit(expansionKey, executionProcessId, content);
    }
  };

  const handleReset = () => {
    if (!readOnly && executionProcessId) {
      resetProcess(executionProcessId);
    }
  };

  // Only show actions when we have a process ID and not already in edit mode
  const canShowActions =
    !readOnly && !!executionProcessId && !isInEditMode && !isResetPending;
  // Edit/retry/reset is not supported when the executor doesn't have the fork capability
  const canEdit = canShowActions && executorCanFork;
  // Only show reset if we have a process ID, not in edit mode, and not pending
  const canReset = Boolean(
    executionProcessId && canEdit && canResetProcess(executionProcessId)
  );

  return (
    <ChatUserMessage
      content={content}
      expanded={expanded}
      onToggle={toggle}
      workspaceId={workspaceId}
      onEdit={canEdit ? handleEdit : undefined}
      onReset={canReset ? handleReset : undefined}
      isGreyed={isGreyed}
      renderMarkdown={({ content, workspaceId }) => (
        <AppChatMarkdown
          content={content}
          workspaceId={workspaceId}
          sessionId={sessionId}
          className={undefined}
          maxWidth={undefined}
        />
      )}
    />
  );
}

/**
 * User feedback entry for denied tool calls
 */
function UserFeedbackEntry({
  content,
  deniedTool,
  workspaceId,
  sessionId,
}: {
  content: string;
  deniedTool: string;
  workspaceId: string | undefined;
  sessionId: string | undefined;
}) {
  const { t } = useTranslation('common');

  return (
    <div className="py-2">
      <div className="bg-background px-4 py-2 text-sm border-y border-dashed">
        <div
          className="text-xs mb-1 opacity-70"
          style={{ color: 'hsl(var(--destructive))' }}
        >
          {t('conversation.deniedByUser', { toolName: deniedTool })}
        </div>
        <WYSIWYGEditor
          value={content}
          disabled
          className="whitespace-pre-wrap break-words flex flex-col gap-1 font-light py-3"
          workspaceId={workspaceId}
          sessionId={sessionId}
        />
      </div>
    </div>
  );
}

/**
 * User answered questions entry with expandable Q&A list
 */
function UserAnsweredQuestionsEntry({
  answers,
  expansionKey,
}: {
  answers: Extract<
    NormalizedEntry['entry_type'],
    { type: 'user_answered_questions' }
  >['answers'];
  expansionKey: string;
}) {
  const { t } = useTranslation('common');
  const [expanded, toggle] = usePersistedExpanded(
    `entry:${expansionKey}`,
    false
  );

  return (
    <div className="py-2">
      <div className="bg-background px-4 py-2 text-sm border-y border-dashed">
        <button
          onClick={() => toggle()}
          className="flex items-center gap-1 text-xs opacity-70 w-full cursor-pointer"
        >
          <CaretDownIcon
            className={cn(
              'size-3 transition-transform',
              !expanded && '-rotate-90'
            )}
          />
          <span>
            {t('askQuestion.answeredCount', { count: answers.length })}
          </span>
        </button>
        {expanded &&
          answers.map((qa, i) => (
            <div key={i} className="mt-2">
              <div className="font-semibold text-sm">{qa.question}</div>
              <div className="text-sm font-light">{qa.answer.join(', ')}</div>
            </div>
          ))}
      </div>
    </div>
  );
}

/**
 * Loading placeholder entry
 */
function LoadingEntry() {
  return (
    <div className="px-4 py-2 text-sm">
      <div className="flex animate-pulse space-x-2 items-center">
        <div className="size-3 bg-foreground/10" />
        <div className="flex-1 h-3 bg-foreground/10" />
        <div className="flex-1 h-3" />
        <div className="flex-1 h-3" />
      </div>
    </div>
  );
}

/**
 * Assistant message entry with expandable content
 */
function AssistantMessageEntry({
  content,
  workspaceId,
  sessionId,
}: {
  content: string;
  workspaceId: string | undefined;
  sessionId: string | undefined;
}) {
  return (
    <ChatAssistantMessage
      content={content}
      workspaceId={workspaceId}
      renderMarkdown={({ content, workspaceId }) => (
        <AppChatMarkdown
          content={content}
          workspaceId={workspaceId}
          sessionId={sessionId}
          className={undefined}
          maxWidth={undefined}
        />
      )}
    />
  );
}

/**
 * Tool summary entry with collapsible content for multi-line summaries
 */
function ToolSummaryEntry({
  summary,
  expansionKey,
  status,
  active,
  content,
  toolName,
  command,
  actionType,
  startedAt,
  endedAt,
}: {
  summary: string;
  expansionKey: string;
  status: ToolStatus;
  active: boolean;
  content: string;
  toolName: string;
  command: string | undefined;
  actionType: string;
  startedAt: string | null;
  endedAt: string | null;
}) {
  const [expanded, toggle] = usePersistedExpanded(
    `tool:${expansionKey}`,
    false
  );
  const { viewToolContentInPanel } = useLogsPanelActions();
  const textRef = useRef<HTMLSpanElement>(null);
  const [isTruncated, setIsTruncated] = useState(false);

  useLayoutEffect(() => {
    const el = textRef.current;
    if (el && !expanded) {
      setIsTruncated(el.scrollWidth > el.clientWidth);
    }
  }, [summary, expanded]);

  // Any tool with output can open the logs panel
  const hasOutput = content && content.trim().length > 0;

  const handleViewContent = useCallback(() => {
    viewToolContentInPanel(toolName, content, command);
  }, [viewToolContentInPanel, toolName, content, command]);

  return (
    <ChatToolSummary
      ref={textRef}
      summary={summary}
      expanded={expanded}
      onToggle={toggle}
      status={status}
      active={active}
      onViewContent={hasOutput ? handleViewContent : undefined}
      toolName={toolName}
      isTruncated={isTruncated}
      actionType={actionType}
      startedAt={startedAt}
      endedAt={endedAt}
    />
  );
}

/**
 * Todo management entry with expandable list of todos
 */
function TodoManagementEntry({
  todos,
  expansionKey,
}: {
  todos: TodoItem[];
  expansionKey: string;
}) {
  const [expanded, toggle] = usePersistedExpanded(
    `todo:${expansionKey}`,
    false
  );

  return <ChatTodoList todos={todos} expanded={expanded} onToggle={toggle} />;
}

/**
 * Subagent/Task entry with expandable output
 */
function SubagentEntry({
  description,
  subagentType,
  result,
  expansionKey,
  status,
  active,
  workspaceId,
  sessionId,
}: {
  description: string;
  subagentType: string | null | undefined;
  result: ToolResult | null | undefined;
  expansionKey: string;
  status: ToolStatus;
  active: boolean;
  workspaceId: string | undefined;
  sessionId: string | undefined;
}) {
  // Only auto-expand if there's a result to show
  const hasResult = Boolean(result?.value);
  const [expanded, toggle] = usePersistedExpanded(
    `subagent:${expansionKey}`,
    false
  );

  return (
    <ChatSubagentEntry
      description={description}
      subagentType={subagentType}
      result={result}
      expanded={expanded}
      onToggle={hasResult ? toggle : undefined}
      status={status}
      active={active}
      workspaceId={workspaceId}
      renderMarkdown={({ content, workspaceId }) => (
        <AppChatMarkdown
          content={content}
          workspaceId={workspaceId}
          sessionId={sessionId}
          className={undefined}
          maxWidth={undefined}
        />
      )}
    />
  );
}

/**
 * System message entry with expandable content
 */
function SystemMessageEntry({
  content,
  expansionKey,
}: {
  content: string;
  expansionKey: string;
}) {
  const [expanded, toggle] = usePersistedExpanded(
    `system:${expansionKey}`,
    false
  );

  return (
    <ChatSystemMessage
      content={content}
      expanded={expanded}
      onToggle={toggle}
    />
  );
}

/**
 * Script entry with fix button for failed scripts
 */
function ScriptEntryWithFix({
  title,
  command,
  processId,
  exitCode,
  status,
  active,
  workspaceId,
  sessionId,
  repos,
  readOnly,
}: {
  title: string;
  command?: string;
  processId: string;
  exitCode: number | null;
  status: ToolStatus;
  active: boolean;
  workspaceId: string | undefined;
  sessionId: string | undefined;
  repos: RepoWithTargetBranch[];
  readOnly: boolean;
}) {
  const { viewProcessInPanel } = useLogsPanelActions();

  const reposRef = useRef(repos);
  reposRef.current = repos;

  const handleFix = useCallback(() => {
    const currentRepos = reposRef.current;
    if (readOnly || !workspaceId || currentRepos.length === 0) return;

    // Determine script type based on title
    const scriptType: ScriptType =
      title === 'Setup Script'
        ? 'setup'
        : title === 'Cleanup Script'
          ? 'cleanup'
          : title === 'Archive Script'
            ? 'archive'
            : 'dev_server';

    ScriptFixerDialog.show({
      scriptType,
      repos: currentRepos,
      workspaceId,
      sessionId,
      initialRepoId: currentRepos.length === 1 ? currentRepos[0].id : undefined,
    });
  }, [readOnly, title, workspaceId, sessionId]);

  // Only show fix button if we have the necessary context
  const canFix = !readOnly && workspaceId && repos.length > 0;

  return (
    <ChatScriptEntry
      title={title}
      command={command}
      processId={processId}
      exitCode={exitCode}
      status={status}
      active={active}
      onViewProcess={viewProcessInPanel}
      onFix={canFix ? handleFix : undefined}
    />
  );
}

/**
 * Error message entry with expandable content
 */
function ErrorMessageEntry({
  content,
  expansionKey,
}: {
  content: string;
  expansionKey: string;
}) {
  const [expanded, toggle] = usePersistedExpanded(
    `error:${expansionKey}`,
    false
  );

  return (
    <ChatErrorMessage content={content} expanded={expanded} onToggle={toggle} />
  );
}

/**
 * Aggregated group entry for consecutive file_read, search, or web_fetch entries
 */
function AggregatedGroupEntry({
  group,
  executionProcess,
}: {
  group: AggregatedPatchGroup;
  executionProcess: ExecutionProcess | null;
}) {
  const { t } = useTranslation('tasks');
  const [expanded, toggle] = usePersistedExpanded(
    `tool:${group.patchKey}`,
    false
  );
  const [isHovered, setIsHovered] = useState(false);

  // Extract summary and status from each entry in the group
  const aggregatedEntries = useMemo(() => {
    return group.entries.map((patchEntry) => {
      if (patchEntry.type !== 'NORMALIZED_ENTRY') {
        const timing = getPatchEntryTiming({
          entry: patchEntry,
          isRunning: false,
          executionProcess,
        });
        return {
          summary: '',
          status: undefined,
          active: false,
          expansionKey: patchEntry.patchKey,
          content: '',
          toolName: '',
          command: undefined,
          ...timing,
        };
      }

      const entryType = patchEntry.content.entry_type;
      if (entryType.type !== 'tool_use') {
        const timing = getPatchEntryTiming({
          entry: patchEntry,
          isRunning: false,
          executionProcess,
        });
        return {
          summary: '',
          status: undefined,
          active: false,
          expansionKey: patchEntry.patchKey,
          content: '',
          toolName: '',
          command: undefined,
          ...timing,
        };
      }

      const { action_type, status, tool_name } = entryType;
      const active = isActiveToolStatus(
        status,
        executionProcess,
        patchEntry.canonical?.active
      );
      const timing = getPatchEntryTiming({
        entry: patchEntry,
        isRunning: active,
        executionProcess,
      });
      let summary = '';
      let content = patchEntry.content.content;
      let command: string | undefined;
      if (action_type.action === 'file_read') {
        summary = action_type.path;
      } else if (action_type.action === 'search') {
        summary = action_type.query;
      } else if (action_type.action === 'web_fetch') {
        summary = action_type.url;
      } else if (action_type.action === 'command_run') {
        summary = action_type.command;
        command = action_type.command;
        content = action_type.result?.output ?? '';
      }

      return {
        summary,
        status,
        active,
        expansionKey: patchEntry.patchKey,
        content,
        toolName: tool_name,
        command,
        ...timing,
      };
    });
  }, [executionProcess, group.entries]);

  const handleToggle = useCallback(() => {
    toggle();
  }, [toggle]);

  const handleHoverChange = useCallback((hovered: boolean) => {
    setIsHovered(hovered);
  }, []);

  const getDisplayIcon = () => {
    switch (group.aggregationType) {
      case 'file_read':
      case 'command_run_read':
        return FileTextIcon;
      case 'web_fetch':
      case 'command_run_fetch':
        return GlobeIcon;
      case 'command_run_edit':
        return PencilSimpleIcon;
      case 'command_run':
      case 'command_run_other':
        return TerminalWindowIcon;
      default:
        return ListMagnifyingGlassIcon;
    }
  };
  const icon = getDisplayIcon();
  const latest = [...aggregatedEntries].reverse();
  const selected =
    latest.find((entry) => entry.active) ??
    latest.find((entry) =>
      ['failed', 'denied', 'timed_out'].includes(entry.status?.status ?? '')
    ) ??
    latest[0];
  const summary = t('conversation.toolGroupLabel', {
    ns: 'common',
    defaultValue: 'Tool activity',
  });
  const detail = selected?.summary || selected?.toolName || undefined;
  const timing = getPatchGroupTiming({
    entries: group.entries,
    isRunning: group.isRunning,
    executionProcess,
  });

  return (
    <ChatAggregatedToolEntries
      entries={aggregatedEntries}
      expanded={expanded}
      isHovered={isHovered}
      onToggle={handleToggle}
      onHoverChange={handleHoverChange}
      summary={summary}
      detail={detail}
      icon={icon}
      forceCollapsible={group.aggregationType === 'tool_calls'}
      startedAt={timing.startedAt}
      endedAt={timing.endedAt}
    />
  );
}

/**
 * Aggregated thinking group entry for thinking entries in previous turns
 */
function AggregatedThinkingGroupEntry({
  group,
  executionProcess,
  workspaceId,
  sessionId,
}: {
  group: AggregatedThinkingGroup;
  executionProcess: ExecutionProcess | null;
  workspaceId: string | undefined;
  sessionId: string | undefined;
}) {
  const [expanded, toggle] = usePersistedExpanded(
    `entry:${group.patchKey}`,
    false
  );
  const [isHovered, setIsHovered] = useState(false);

  // Extract thinking entries from the group
  const thinkingEntries = useMemo(() => {
    return group.entries
      .filter((entry) => entry.type === 'NORMALIZED_ENTRY')
      .map((entry) => ({
        content: entry.type === 'NORMALIZED_ENTRY' ? entry.content.content : '',
        expansionKey: entry.patchKey,
        ...getPatchEntryTiming({
          entry,
          isRunning: false,
          executionProcess,
        }),
      }));
  }, [executionProcess, group.entries]);

  const handleToggle = useCallback(() => {
    toggle();
  }, [toggle]);

  const handleHoverChange = useCallback((hovered: boolean) => {
    setIsHovered(hovered);
  }, []);
  const timing = getPatchGroupTiming({
    entries: group.entries,
    isRunning: false,
    executionProcess,
  });

  return (
    <ChatCollapsedThinking
      entries={thinkingEntries}
      expanded={expanded}
      isHovered={isHovered}
      onToggle={handleToggle}
      onHoverChange={handleHoverChange}
      workspaceId={workspaceId}
      startedAt={timing.startedAt}
      endedAt={timing.endedAt}
      renderMarkdown={({ content, workspaceId, className }) => (
        <AppChatMarkdown
          content={content}
          workspaceId={workspaceId}
          sessionId={sessionId}
          className={className}
          maxWidth={undefined}
        />
      )}
    />
  );
}

function AggregatedFileChangeGroupEntry({
  group,
  executionProcess,
  workspaceId,
  sessionId,
  repos,
}: {
  group: AggregatedFileChangeGroup;
  executionProcess: ExecutionProcess | null;
  workspaceId: string | undefined;
  sessionId: string | undefined;
  repos: RepoWithTargetBranch[];
}) {
  const { theme } = useTheme();
  const actualTheme = getActualTheme(theme);
  const { viewFileInChanges, hasDiffPath } = useChangesViewActions();
  const { canOpenWorkspaceFilePreview, openWorkspaceFilePreview } =
    useWorkspaceFilePreviewActions();
  const resolveFilePreviewTarget = useWorkspaceFilePreviewResolver(
    workspaceId,
    repos,
    sessionId
  );
  const [expanded, toggle] = usePersistedExpanded(
    `file-change:${group.patchKey}` as PersistKey,
    false
  );
  const [isHovered, setIsHovered] = useState(false);
  const isVSCode = inIframe();

  const aggregatedFileChangeEntries = useMemo(() => {
    return group.entries.flatMap((patchEntry, entryIdx) => {
      if (patchEntry.type !== 'NORMALIZED_ENTRY') {
        return [];
      }

      const entryType = patchEntry.content.entry_type;
      if (entryType.type !== 'tool_use') {
        return [];
      }

      const { action_type, status } = entryType;
      if (action_type.action !== 'file_edit') {
        return [];
      }

      return action_type.changes.map((change, changeIdx) => {
        const filePath =
          change.action === 'rename' && change.new_path
            ? change.new_path
            : action_type.path;
        const previewResolution = resolveFilePreviewTarget({
          path: filePath,
          source: 'diff',
        });

        return {
          filePath,
          change,
          status,
          active: isActiveToolStatus(status, executionProcess),
          expansionKey: `${patchEntry.patchKey}:${entryIdx}:${changeIdx}`,
          fileIcon: getFileIcon(filePath, actualTheme),
          onOpenInVSCode: () =>
            openFileInVSCode(filePath, { openAsDiff: false }),
          onOpenInChanges: hasDiffPath(filePath)
            ? () => viewFileInChanges(filePath)
            : undefined,
          onOpenFilePreview:
            canOpenWorkspaceFilePreview &&
            previewResolution.status === 'resolved'
              ? () => openWorkspaceFilePreview(previewResolution.target)
              : undefined,
        };
      });
    });
  }, [
    actualTheme,
    canOpenWorkspaceFilePreview,
    executionProcess,
    group.entries,
    hasDiffPath,
    openWorkspaceFilePreview,
    resolveFilePreviewTarget,
    viewFileInChanges,
  ]);

  const handleToggle = useCallback(() => {
    toggle();
  }, [toggle]);

  const handleHoverChange = useCallback((hovered: boolean) => {
    setIsHovered(hovered);
  }, []);

  return (
    <ChatAggregatedFileChangeEntries
      entries={aggregatedFileChangeEntries}
      expanded={expanded}
      isHovered={isHovered}
      onToggle={handleToggle}
      onHoverChange={handleHoverChange}
      isVSCode={isVSCode}
      renderDiffBody={({ diffContent }) =>
        diffContent ? <FileEntryDiffBody diffContent={diffContent} /> : null
      }
    />
  );
}

function AggregatedDiffGroupEntry({
  group,
  executionProcess,
  workspaceId,
  sessionId,
  repos,
}: {
  group: AggregatedDiffGroup;
  executionProcess: ExecutionProcess | null;
  workspaceId: string | undefined;
  sessionId: string | undefined;
  repos: RepoWithTargetBranch[];
}) {
  const { theme } = useTheme();
  const actualTheme = getActualTheme(theme);
  const { viewFileInChanges, hasDiffPath } = useChangesViewActions();
  const { canOpenWorkspaceFilePreview, openWorkspaceFilePreview } =
    useWorkspaceFilePreviewActions();
  const resolveFilePreviewTarget = useWorkspaceFilePreviewResolver(
    workspaceId,
    repos,
    sessionId
  );
  const [expanded, toggle] = usePersistedExpanded(
    `diff:${group.patchKey}`,
    false
  );
  const [isHovered, setIsHovered] = useState(false);
  const FileIcon = useMemo(
    () => getFileIcon(group.filePath, actualTheme),
    [group.filePath, actualTheme]
  );
  const isVSCode = inIframe();

  // Extract change data and status from each entry
  const aggregatedDiffEntries = useMemo(() => {
    return group.entries.flatMap((patchEntry, entryIdx) => {
      if (patchEntry.type !== 'NORMALIZED_ENTRY') {
        return [];
      }

      const entryType = patchEntry.content.entry_type;
      if (entryType.type !== 'tool_use') {
        return [];
      }

      const { action_type, status } = entryType;
      if (action_type.action !== 'file_edit') {
        return [];
      }

      // Each file_edit entry can have multiple changes
      return action_type.changes.map((change, changeIdx) => ({
        change,
        status,
        active: isActiveToolStatus(status, executionProcess),
        expansionKey: `${patchEntry.patchKey}:${entryIdx}:${changeIdx}`,
      }));
    });
  }, [executionProcess, group.entries]);

  const handleToggle = useCallback(() => {
    toggle();
  }, [toggle]);

  const handleHoverChange = useCallback((hovered: boolean) => {
    setIsHovered(hovered);
  }, []);

  const handleOpenInChanges = useCallback(() => {
    if (!hasDiffPath(group.filePath)) return;
    viewFileInChanges(group.filePath);
  }, [viewFileInChanges, hasDiffPath, group.filePath]);
  const previewResolution = useMemo(
    () =>
      resolveFilePreviewTarget({
        path: group.filePath,
        source: 'diff',
      }),
    [group.filePath, resolveFilePreviewTarget]
  );
  const handleOpenFilePreview = useCallback(() => {
    if (
      !canOpenWorkspaceFilePreview ||
      previewResolution.status !== 'resolved'
    ) {
      return;
    }
    openWorkspaceFilePreview(previewResolution.target);
  }, [
    canOpenWorkspaceFilePreview,
    openWorkspaceFilePreview,
    previewResolution,
  ]);
  const handleOpenInVSCode = useCallback((filePath: string) => {
    openFileInVSCode(filePath, { openAsDiff: false });
  }, []);

  return (
    <ChatAggregatedDiffEntries
      filePath={group.filePath}
      entries={aggregatedDiffEntries}
      expanded={expanded}
      isHovered={isHovered}
      onToggle={handleToggle}
      onHoverChange={handleHoverChange}
      onOpenFilePreview={
        canOpenWorkspaceFilePreview && previewResolution.status === 'resolved'
          ? handleOpenFilePreview
          : undefined
      }
      onOpenInChanges={handleOpenInChanges}
      fileIcon={FileIcon}
      isVSCode={isVSCode}
      onOpenInVSCode={handleOpenInVSCode}
      renderDiffBody={({ diffContent }) =>
        diffContent ? <FileEntryDiffBody diffContent={diffContent} /> : null
      }
    />
  );
}

const DisplayConversationEntrySpaced = (props: Props) => {
  const { isEntryGreyed } = useMessageEditContext();
  const isGreyed = isEntryGreyed(props.expansionKey);

  return (
    <div
      className={cn(
        'py-base px-double',
        isGreyed && 'opacity-50 pointer-events-none'
      )}
    >
      <DisplayConversationEntry {...props} />
    </div>
  );
};

export default DisplayConversationEntrySpaced;
