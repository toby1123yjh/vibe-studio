import {
  AgentRunStatus,
  ExecutionProcess,
  ExecutorAction,
  PatchType,
  Workspace,
} from 'shared/types';

export interface CanonicalConversationIdentity {
  readonly agentRunId: string;
  readonly runAttemptId: string;
  readonly runAttemptNumber: number | null;
  readonly eventId: string;
  readonly eventIds: readonly string[];
  readonly sequence: bigint;
  readonly active: boolean;
}

export type PatchTypeWithKey = PatchType & {
  patchKey: string;
  executionProcessId?: string;
  canonical?: CanonicalConversationIdentity;
  /** Stable read-model reference. Text and tool details subscribe independently. */
  conversationBlockId?: string;
  conversationParentBlockId?: string | null;
};

/**
 * Aggregation types for tool use entries that can be grouped together.
 */
export type ToolAggregationType =
  | 'tool_calls'
  | 'file_read'
  | 'search'
  | 'web_fetch'
  | 'command_run'
  | 'command_run_read'
  | 'command_run_search'
  | 'command_run_edit'
  | 'command_run_fetch'
  | 'command_run_other';

/**
 * A group of consecutive entries of the same aggregatable type (e.g., file_read, search, web_fetch).
 * Used to display multiple read/search/fetch operations in a collapsed accordion style.
 */
export type AggregatedPatchGroup = {
  type: 'AGGREGATED_GROUP';
  /** The aggregation category (e.g., 'file_read', 'search', 'web_fetch') */
  aggregationType: ToolAggregationType;
  /** Whether any entry in this group is still active or awaiting approval */
  isRunning: boolean;
  /** The individual entries in this group */
  entries: PatchTypeWithKey[];
  /** Unique key for the group */
  patchKey: string;
  executionProcessId?: string;
};

/**
 * A group of consecutive file_edit entries for the same file path.
 * Used to display multiple edits to the same file in a collapsed accordion style.
 */
export type AggregatedDiffGroup = {
  type: 'AGGREGATED_DIFF_GROUP';
  /** The file path being edited */
  filePath: string;
  /** The individual file_edit entries in this group */
  entries: PatchTypeWithKey[];
  /** Unique key for the group */
  patchKey: string;
  executionProcessId?: string;
};

/**
 * A group of consecutive file_edit entries across one or more file paths.
 * Used to collapse noisy multi-file edit runs into one activity row.
 */
export type AggregatedFileChangeGroup = {
  type: 'AGGREGATED_FILE_CHANGE_GROUP';
  /** The individual file_edit entries in this consecutive run */
  entries: PatchTypeWithKey[];
  /** Unique key for the group */
  patchKey: string;
  executionProcessId?: string;
};

/**
 * A group of thinking entries from a previous conversation turn.
 * Used to collapse thinking steps in previous answers for cleaner display.
 */
export type AggregatedThinkingGroup = {
  type: 'AGGREGATED_THINKING_GROUP';
  /** The individual thinking entries in this group */
  entries: PatchTypeWithKey[];
  /** Unique key for the group */
  patchKey: string;
  executionProcessId?: string;
};

export type DisplayEntry =
  | PatchTypeWithKey
  | AggregatedPatchGroup
  | AggregatedDiffGroup
  | AggregatedFileChangeGroup
  | AggregatedThinkingGroup;

export function isAggregatedGroup(
  entry: DisplayEntry
): entry is AggregatedPatchGroup {
  return entry.type === 'AGGREGATED_GROUP';
}

export function isAggregatedDiffGroup(
  entry: DisplayEntry
): entry is AggregatedDiffGroup {
  return entry.type === 'AGGREGATED_DIFF_GROUP';
}

export function isAggregatedThinkingGroup(
  entry: DisplayEntry
): entry is AggregatedThinkingGroup {
  return entry.type === 'AGGREGATED_THINKING_GROUP';
}

export function isAggregatedFileChangeGroup(
  entry: DisplayEntry
): entry is AggregatedFileChangeGroup {
  return entry.type === 'AGGREGATED_FILE_CHANGE_GROUP';
}

export type AddEntryType = 'initial' | 'running' | 'historic' | 'plan';

export interface ConversationTimelineSource {
  executionProcessState: ExecutionProcessStateStore;
  liveExecutionProcesses: ExecutionProcess[];
  canonical: CanonicalConversationProjection;
}

export interface CanonicalConversationProjection {
  readonly entries: PatchTypeWithKey[];
  readonly activeAgentRunIds: ReadonlySet<string>;
  readonly runCount: number;
  readonly isLoading: boolean;
  readonly isRunning: boolean;
  readonly projectionDegraded: boolean;
  readonly latestStatus: AgentRunStatus | null;
}

export function getConversationEntryRuntimeId(
  entry: Pick<PatchTypeWithKey, 'canonical' | 'executionProcessId'>
): string | null {
  return entry.canonical?.agentRunId ?? entry.executionProcessId ?? null;
}

export type OnEntriesUpdated = (
  newEntries: PatchTypeWithKey[],
  addType: AddEntryType,
  loading: boolean
) => void;

export type OnTimelineUpdated = (
  source: ConversationTimelineSource,
  addType: AddEntryType,
  loading: boolean
) => void;

export type ExecutionProcessStaticInfo = {
  id: string;
  created_at: string;
  updated_at: string;
  executor_action: ExecutorAction;
};

export type ExecutionProcessState = {
  executionProcess: ExecutionProcessStaticInfo;
  entries: PatchTypeWithKey[];
};

export type ExecutionProcessStateStore = Record<string, ExecutionProcessState>;

export interface UseConversationHistoryParams {
  attempt: Workspace;
  onTimelineUpdated?: OnTimelineUpdated;
  onEntriesUpdated?: OnEntriesUpdated;
  scopeKey: string;
}

export interface UseConversationHistoryResult {}
