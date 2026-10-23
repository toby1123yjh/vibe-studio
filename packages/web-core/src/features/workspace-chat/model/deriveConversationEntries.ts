import { NormalizedEntry, PatchType, TokenUsageInfo } from 'shared/types';

import { nextActionPatch } from '@/shared/hooks/useConversationHistory/constants';
import type { PatchTypeWithKey } from '@/shared/hooks/useConversationHistory/types';
import {
  deriveConversationTurns,
  type ConversationScriptTurn,
} from './deriveConversationTurns';
import { mergeWorkflowNotifications } from '@/features/workflow/model/workflowNotifications';

export interface DerivedConversationEntriesResult {
  readonly entries: PatchTypeWithKey[];
  readonly hasRunningProcess: boolean;
  readonly latestTokenUsageInfo: TokenUsageInfo | null;
}

interface DeriveConversationEntriesParams {
  readonly source: import('@/shared/hooks/useConversationHistory/types').ConversationTimelineSource;
  readonly scriptOutputCache: Map<string, { count: number; output: string }>;
  readonly workflowSystemEntries?: readonly PatchTypeWithKey[];
}

function patchWithKey(
  patch: PatchType,
  executionProcessId: string,
  index: number | 'user' | 'script'
): PatchTypeWithKey {
  return {
    ...patch,
    patchKey: `${executionProcessId}:${index}`,
    executionProcessId,
  };
}

function appendScriptTurnEntries(
  turn: ConversationScriptTurn,
  turnEntries: PatchTypeWithKey[],
  scriptOutputCache: Map<string, { count: number; output: string }>
) {
  for (const process of turn.processes) {
    const processId = process.process.executionProcess.id;
    const entryCount = process.process.rawEntries.length;
    const cachedOutput = scriptOutputCache.get(processId);
    const output =
      cachedOutput && cachedOutput.count === entryCount
        ? cachedOutput.output
        : process.process.rawEntries.map((entry) => entry.content).join('\n');

    scriptOutputCache.set(processId, {
      count: entryCount,
      output,
    });

    const scriptAction = process.process.executionProcess.executor_action.typ;
    if (scriptAction.type !== 'ScriptRequest') {
      continue;
    }

    const toolNormalizedEntry: NormalizedEntry = {
      entry_type: {
        type: 'tool_use',
        tool_name: process.toolName,
        action_type: {
          action: 'command_run',
          command: scriptAction.script,
          result: {
            output,
            exit_status: process.exitStatus,
          },
          category: 'other',
        },
        status: process.toolStatus,
      },
      content: process.toolName,
      timestamp: null,
    };

    turnEntries.push(
      patchWithKey(
        { type: 'NORMALIZED_ENTRY', content: toolNormalizedEntry },
        processId,
        'script'
      )
    );

    if (
      process.shouldEmitInitialPromptAfterSetup &&
      process.initialPromptAfterSetup
    ) {
      turnEntries.push(
        patchWithKey(
          {
            type: 'NORMALIZED_ENTRY',
            content: {
              entry_type: { type: 'user_message' },
              content: process.initialPromptAfterSetup,
              timestamp: null,
            },
          },
          processId,
          'user'
        )
      );
    }
  }
}

// This stage serializes already-derived turn meaning into visible conversation entries.

export function deriveConversationEntries({
  source,
  scriptOutputCache,
  workflowSystemEntries = [],
}: DeriveConversationEntriesParams): DerivedConversationEntriesResult {
  const conversationTurns = deriveConversationTurns(source);

  let hasPendingApproval = false;
  let hasRunningProcess = false;
  let lastProcessFailedOrKilled = false;
  let latestTokenUsageInfo: TokenUsageInfo | null = null;

  const scriptEntries = conversationTurns.turns.flatMap((turn, index) => {
    const turnEntries: PatchTypeWithKey[] = [];

    if (turn.processes.some((process) => process.process.isRunning)) {
      hasRunningProcess = true;
    }

    if (
      turn.processes.some((process) => process.process.failedOrKilled) &&
      index === conversationTurns.turns.length - 1
    ) {
      lastProcessFailedOrKilled = true;
    }

    appendScriptTurnEntries(turn, turnEntries, scriptOutputCache);
    return turnEntries;
  });

  const setupEntries: PatchTypeWithKey[] = [];
  const trailingScriptEntries: PatchTypeWithKey[] = [];
  for (const entry of scriptEntries) {
    const process =
      source.executionProcessState[entry.executionProcessId ?? ''];
    const action = process?.executionProcess.executor_action.typ;
    if (
      action?.type === 'ScriptRequest' &&
      (action.context === 'SetupScript' ||
        action.context === 'ToolInstallScript')
    ) {
      setupEntries.push(entry);
    } else {
      trailingScriptEntries.push(entry);
    }
  }

  const entries = mergeWorkflowNotifications(
    [...setupEntries, ...source.canonical.entries, ...trailingScriptEntries],
    workflowSystemEntries
  );
  hasRunningProcess ||= source.canonical.isRunning;
  hasPendingApproval ||= source.canonical.entries.some(
    (entry) =>
      entry.type === 'NORMALIZED_ENTRY' &&
      entry.content.entry_type.type === 'tool_use' &&
      entry.content.entry_type.status.status === 'pending_approval'
  );
  const canonicalTokenUsage = source.canonical.entries.findLast(
    (entry) =>
      entry.type === 'NORMALIZED_ENTRY' &&
      entry.content.entry_type.type === 'token_usage_info'
  );
  if (
    canonicalTokenUsage?.type === 'NORMALIZED_ENTRY' &&
    canonicalTokenUsage.content.entry_type.type === 'token_usage_info'
  ) {
    latestTokenUsageInfo = canonicalTokenUsage.content.entry_type;
  }

  if (!hasRunningProcess && !hasPendingApproval) {
    entries.push(
      nextActionPatch(
        lastProcessFailedOrKilled,
        conversationTurns.turns.length,
        false,
        undefined
      )
    );
  }

  return {
    entries,
    hasRunningProcess,
    latestTokenUsageInfo,
  };
}
