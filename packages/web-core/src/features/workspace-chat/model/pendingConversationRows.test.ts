import { expect, test } from '@playwright/test';
import type { PatchTypeWithKey } from '@/shared/hooks/useConversationHistory/types';

import { buildConversationRow } from './conversation-row-model';
import { projectPendingConversationRows } from './pendingConversationRows';
import type {
  PendingConversationSend,
  PendingConversationSendSnapshot,
} from './pendingConversationSends';

function entry(runId: string, user = true): PatchTypeWithKey {
  return {
    type: 'NORMALIZED_ENTRY',
    patchKey: `${runId}:${user ? 'user' : 'assistant'}`,
    content: {
      entry_type: { type: user ? 'user_message' : 'assistant_message' },
      content: 'An identical message',
      timestamp: null,
    },
    canonical: {
      agentRunId: runId,
      runAttemptId: `${runId}:attempt`,
      runAttemptNumber: 1,
      eventId: `${runId}:event`,
      eventIds: [`${runId}:event`],
      sequence: 1n,
      active: false,
    },
  };
}

function pending(
  overrides: Partial<PendingConversationSend> = {}
): PendingConversationSend {
  return {
    id: 'local-1',
    entry: {
      ...entry('temporary'),
      canonical: undefined,
      patchKey: 'pending-send:local-1',
    },
    text: 'An identical message',
    executorConfig: null,
    baselineRunIds: new Set(['old']),
    agentRunId: null,
    phase: 'sending',
    restoreDraft: true,
    ...overrides,
  };
}

function snapshot(
  ...sends: PendingConversationSend[]
): PendingConversationSendSnapshot {
  return { sends, error: null, failedDraft: null };
}

test('stream before acknowledgement buffers a new run and preserves an older identical message', () => {
  const old = buildConversationRow(entry('old'));
  const newUser = buildConversationRow(entry('new'));
  const newTools = buildConversationRow({
    type: 'AGGREGATED_GROUP',
    patchKey: 'tool-group:new',
    aggregationType: 'tool_calls',
    isRunning: false,
    entries: [entry('new', false), entry('new', false)],
  });
  const result = projectPendingConversationRows(
    [old, newUser, newTools],
    snapshot(pending()),
    new Map()
  );
  expect(result.rows.map((row) => row.semanticKey)).toEqual([
    old.semanticKey,
    'conv-pending-send:local-1',
  ]);
  expect(result.rows[0]).toBe(old);
  expect(result.rows[1].submissionStatus).toBe('sending');
});

test('acknowledged echo hands its identity to the exact canonical user and survives settlement', () => {
  const old = buildConversationRow(entry('old'));
  const user = buildConversationRow(entry('new'));
  const assistant = buildConversationRow(entry('new', false));
  const accepted = snapshot(pending({ phase: 'accepted', agentRunId: 'new' }));
  const before = projectPendingConversationRows([old], accepted, new Map());
  const after = projectPendingConversationRows(
    [old, user, assistant],
    accepted,
    before.aliases
  );
  expect(after.rows.map((row) => row.semanticKey)).toEqual([
    old.semanticKey,
    before.rows[1].semanticKey,
    assistant.semanticKey,
  ]);
  expect(after.rows[1].entry).toBe(user.entry);
  expect(after.rows[1].submissionStatus).toBeUndefined();
  const settled = projectPendingConversationRows(
    [old, user, assistant],
    snapshot(),
    after.aliases
  );
  expect(settled.rows.map((row) => row.semanticKey)).toEqual(
    after.rows.map((row) => row.semanticKey)
  );
});

test('an accepted prompt missing from history stays before its own output', () => {
  const output = buildConversationRow(entry('new', false));
  const result = projectPendingConversationRows(
    [output],
    snapshot(pending({ phase: 'accepted', agentRunId: 'new' })),
    new Map()
  );
  expect(result.rows.map((row) => row.rowFamily)).toEqual([
    'user_message',
    'assistant_message',
  ]);
  expect(result.rows[1]).toBe(output);
});

test('sending reuses all 500 stable history rows and bounds aliases to the current window', () => {
  const rows = Array.from({ length: 500 }, (_, index) =>
    buildConversationRow(entry(`old-${index}`))
  );
  const send = pending({
    baselineRunIds: new Set(
      Array.from({ length: 500 }, (_, index) => `old-${index}`)
    ),
  });
  const result = projectPendingConversationRows(
    rows,
    snapshot(send),
    new Map([['outside-window', 'conv-pending-send:expired']])
  );
  expect(result.rows).toHaveLength(501);
  expect(rows.every((row, index) => result.rows[index] === row)).toBe(true);
  expect(result.aliases.size).toBe(0);
});
