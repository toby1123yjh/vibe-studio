import type { DisplayEntry } from '@/shared/hooks/useConversationHistory/types';

import {
  buildConversationRow,
  type ConversationRow,
} from './conversation-row-model';
import type { PendingConversationSendSnapshot } from './pendingConversationSends';

function runIdForEntry(entry: DisplayEntry): string | undefined {
  return 'entries' in entry
    ? entry.entries[0]?.canonical?.agentRunId
    : entry.canonical?.agentRunId;
}

/**
 * Local echoes sit on top of the already-derived canonical timeline. Submitting
 * or editing the next draft must not repeat grouping, Markdown projection, or
 * script-history derivation before the user's message can paint.
 */
export function projectPendingConversationRows(
  canonicalRows: ConversationRow[],
  snapshot: PendingConversationSendSnapshot,
  previousAliases: ReadonlyMap<string, string>
): {
  rows: ConversationRow[];
  aliases: ReadonlyMap<string, string>;
} {
  if (!snapshot.sends.length && !previousAliases.size)
    return { rows: canonicalRows, aliases: previousAliases };

  const aliases = new Map(previousAliases);
  const retainedRunIds = new Set<string>();
  for (const row of canonicalRows) {
    const runId = runIdForEntry(row.entry);
    if (runId) retainedRunIds.add(runId);
  }
  for (const send of snapshot.sends) {
    if (!send.agentRunId) continue;
    retainedRunIds.add(send.agentRunId);
    aliases.set(send.agentRunId, `conv-${send.entry.patchKey}`);
  }
  // The canonical read model has a bounded window. Do not retain aliases for
  // turns outside that window indefinitely.
  for (const runId of aliases.keys()) {
    if (!retainedRunIds.has(runId)) aliases.delete(runId);
  }

  const sending = snapshot.sends.find((send) => send.phase === 'sending');
  const visibleRows = sending
    ? canonicalRows.filter((row) => {
        const runId = runIdForEntry(row.entry);
        return !runId || sending.baselineRunIds.has(runId);
      })
    : canonicalRows;
  const userRunIds = new Set(
    visibleRows.flatMap((row) => {
      const runId = row.isUserMessage ? runIdForEntry(row.entry) : undefined;
      return runId ? [runId] : [];
    })
  );
  const echoesByRun = new Map<string, ConversationRow[]>();
  const trailingEchoes: ConversationRow[] = [];
  for (const send of snapshot.sends) {
    // Only the acknowledged Run ID can replace an echo, even when an older
    // message has exactly the same text or the stream precedes HTTP.
    if (send.agentRunId && userRunIds.has(send.agentRunId)) continue;
    const row = {
      ...buildConversationRow(send.entry),
      submissionStatus: send.phase,
    };
    if (send.agentRunId) {
      const echoes = echoesByRun.get(send.agentRunId) ?? [];
      echoes.push(row);
      echoesByRun.set(send.agentRunId, echoes);
    } else trailingEchoes.push(row);
  }

  const rows: ConversationRow[] = [];
  for (const row of visibleRows) {
    const runId = runIdForEntry(row.entry);
    if (runId) {
      const echoes = echoesByRun.get(runId);
      if (echoes) {
        rows.push(...echoes);
        echoesByRun.delete(runId);
      }
    }
    const semanticKey = row.isUserMessage && runId ? aliases.get(runId) : null;
    rows.push(
      semanticKey && semanticKey !== row.semanticKey
        ? { ...row, semanticKey }
        : row
    );
  }
  for (const echoes of echoesByRun.values()) rows.push(...echoes);
  rows.push(...trailingEchoes);
  return { rows, aliases };
}
