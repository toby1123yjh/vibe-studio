import type { AgentRunStatus, NormalizedEntry } from 'shared/types';

import type {
  CanonicalConversationIdentity,
  CanonicalConversationProjection,
  PatchTypeWithKey,
} from '@/shared/hooks/useConversationHistory/types';

const SUCCEEDED_STATUS = 'succeeded' as AgentRunStatus;
const CANCELLED_STATUS = 'cancelled' as AgentRunStatus;

export interface AgentWorkbenchTimelineCopy {
  readonly fileChanges: string;
  readonly working: string;
  readonly terminalStatuses: Readonly<Partial<Record<AgentRunStatus, string>>>;
}

const DEFAULT_TIMELINE_COPY: AgentWorkbenchTimelineCopy = {
  fileChanges: 'File changes',
  working: 'Agent is working',
  terminalStatuses: {
    [CANCELLED_STATUS]: 'Agent run cancelled',
    failed: 'Agent run failed',
    crashed: 'Agent run crashed',
    audit_failed: 'Agent run audit failed',
  },
};

export interface AgentWorkbenchTimelineIdentity {
  patchKey: string;
  eventId: string | null;
  eventIds: readonly string[];
  agentRunId: string | null;
  runAttemptId: string | null;
  runAttemptNumber: number | null;
  sequence: bigint | null;
  active: boolean;
  parentBlockId?: string | null;
}

interface TimelineItemBase extends AgentWorkbenchTimelineIdentity {
  timestamp: string | null;
}

export interface AgentWorkbenchMessageItem extends TimelineItemBase {
  kind: 'message';
  role: 'user' | 'assistant' | 'system' | 'thinking';
  content: string;
}

export interface AgentWorkbenchToolItem extends TimelineItemBase {
  kind: 'tool';
  toolName: string;
  content: string;
  entry: Extract<NormalizedEntry['entry_type'], { type: 'tool_use' }>;
}

export interface AgentWorkbenchToolGroupItem extends TimelineItemBase {
  kind: 'tool-group';
  items: readonly AgentWorkbenchToolItem[];
}

export interface AgentWorkbenchInteractionItem extends TimelineItemBase {
  kind: 'interaction';
  interaction: 'approval' | 'input';
  content: string;
  entry: Extract<NormalizedEntry['entry_type'], { type: 'tool_use' }>;
}

export interface AgentWorkbenchFailureItem extends TimelineItemBase {
  kind: 'failure';
  content: string;
  source: 'canonical-error' | 'stderr';
}

export interface AgentWorkbenchStatusItem extends TimelineItemBase {
  kind: 'status';
  status: 'active' | AgentRunStatus;
  content: string;
}

export type AgentWorkbenchTimelineItem =
  | AgentWorkbenchMessageItem
  | AgentWorkbenchToolItem
  | AgentWorkbenchToolGroupItem
  | AgentWorkbenchInteractionItem
  | AgentWorkbenchFailureItem
  | AgentWorkbenchStatusItem;

function identity(
  patchKey: string,
  canonical?: CanonicalConversationIdentity
): AgentWorkbenchTimelineIdentity {
  return {
    patchKey,
    eventId: canonical?.eventId ?? null,
    eventIds: canonical?.eventIds ?? [],
    agentRunId: canonical?.agentRunId ?? null,
    runAttemptId: canonical?.runAttemptId ?? null,
    runAttemptNumber: canonical?.runAttemptNumber ?? null,
    sequence: canonical?.sequence ?? null,
    active: canonical?.active ?? false,
  };
}

function messageRole(
  type: NormalizedEntry['entry_type']['type']
): AgentWorkbenchMessageItem['role'] | null {
  switch (type) {
    case 'user_message':
      return 'user';
    case 'assistant_message':
      return 'assistant';
    case 'system_message':
      return 'system';
    case 'thinking':
      return 'thinking';
    default:
      return null;
  }
}

function projectEntry(
  patch: PatchTypeWithKey,
  copy: AgentWorkbenchTimelineCopy
): Exclude<AgentWorkbenchTimelineItem, AgentWorkbenchToolGroupItem> {
  const base = identity(patch.patchKey, patch.canonical);
  base.parentBlockId = patch.conversationParentBlockId;
  if (patch.type === 'STDERR') {
    return {
      ...base,
      kind: 'failure',
      content: patch.content,
      source: 'stderr',
      timestamp: null,
    };
  }
  if (patch.type !== 'NORMALIZED_ENTRY') {
    return {
      ...base,
      kind: 'status',
      status: base.active ? 'active' : SUCCEEDED_STATUS,
      content: patch.type === 'STDOUT' ? patch.content : copy.fileChanges,
      timestamp: null,
    };
  }

  const entry = patch.content;
  const role = messageRole(entry.entry_type.type);
  if (role) {
    return {
      ...base,
      kind: 'message',
      role,
      content: entry.content,
      timestamp: entry.timestamp,
    };
  }

  if (entry.entry_type.type === 'tool_use') {
    const isInput = entry.entry_type.action_type.action === 'ask_user_question';
    const isApproval = entry.entry_type.status.status === 'pending_approval';
    if (isInput || isApproval) {
      return {
        ...base,
        kind: 'interaction',
        interaction: isInput ? 'input' : 'approval',
        content: entry.content,
        entry: entry.entry_type,
        timestamp: entry.timestamp,
      };
    }
    return {
      ...base,
      kind: 'tool',
      toolName: entry.entry_type.tool_name,
      content: entry.content,
      entry: entry.entry_type,
      timestamp: entry.timestamp,
    };
  }

  if (entry.entry_type.type === 'error_message') {
    return {
      ...base,
      kind: 'failure',
      content: entry.content,
      source: 'canonical-error',
      timestamp: entry.timestamp,
    };
  }

  if (entry.entry_type.type === 'loading') {
    return {
      ...base,
      kind: 'status',
      status: 'active',
      content: copy.working,
      timestamp: entry.timestamp,
    };
  }

  return {
    ...base,
    kind: 'status',
    status: base.active ? 'active' : SUCCEEDED_STATUS,
    content: entry.content,
    timestamp: entry.timestamp,
  };
}

function groupAdjacentTools(
  items: readonly Exclude<
    AgentWorkbenchTimelineItem,
    AgentWorkbenchToolGroupItem
  >[]
): AgentWorkbenchTimelineItem[] {
  const result: AgentWorkbenchTimelineItem[] = [];
  let tools: AgentWorkbenchToolItem[] = [];

  const flush = () => {
    if (tools.length === 1) result.push(tools[0]);
    if (tools.length > 1) {
      const first = tools[0];
      const last = tools.at(-1) ?? first;
      result.push({
        ...first,
        kind: 'tool-group',
        patchKey: first.patchKey.startsWith('conversation-block:')
          ? `tool-group:${first.patchKey}`
          : `tool-group:${tools.map((tool) => tool.patchKey).join(':')}`,
        eventId: last.eventId,
        eventIds: tools.flatMap((tool) => tool.eventIds),
        active: tools.some((tool) => tool.active),
        items: tools,
      });
    }
    tools = [];
  };

  for (const item of items) {
    if (item.kind === 'tool') {
      const previous = tools.at(-1);
      if (
        previous &&
        (previous.agentRunId !== item.agentRunId ||
          previous.runAttemptId !== item.runAttemptId ||
          previous.runAttemptNumber !== item.runAttemptNumber ||
          previous.parentBlockId !== item.parentBlockId)
      ) {
        flush();
      }
      tools.push(item);
      continue;
    }
    flush();
    result.push(item);
  }
  flush();
  return result;
}

export function projectAgentWorkbenchTimeline(
  source: readonly PatchTypeWithKey[] | CanonicalConversationProjection,
  copy: AgentWorkbenchTimelineCopy = DEFAULT_TIMELINE_COPY
): AgentWorkbenchTimelineItem[] {
  const isProjection = 'runCount' in source;
  const entries = isProjection ? source.entries : source;
  const items = groupAdjacentTools(
    entries.map((entry) => projectEntry(entry, copy))
  );

  const terminalContent = isProjection
    ? copy.terminalStatuses[source.latestStatus as AgentRunStatus]
    : undefined;
  if (isProjection && source.latestStatus && terminalContent) {
    // Runtime system messages have no AgentRun identity. A trailing workflow
    // notification must not become the identity of a native terminal status.
    const last = items.findLast((item) => item.agentRunId !== null);
    items.push({
      ...identity(
        `agent-run:${last?.agentRunId ?? 'latest'}:status:${source.latestStatus}`
      ),
      eventId: last?.eventId ?? null,
      eventIds: last?.eventIds ?? [],
      agentRunId: last?.agentRunId ?? null,
      runAttemptId: last?.runAttemptId ?? null,
      runAttemptNumber: last?.runAttemptNumber ?? null,
      sequence: last?.sequence ?? null,
      kind: 'status',
      status: source.latestStatus,
      content: terminalContent,
      timestamp: last?.timestamp ?? null,
    });
  }
  return items;
}
