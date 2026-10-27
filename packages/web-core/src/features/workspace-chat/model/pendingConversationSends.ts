import { useCallback, useSyncExternalStore } from 'react';
import type { ExecutorConfig } from 'shared/types';
import type { PatchTypeWithKey } from '@/shared/hooks/useConversationHistory/types';

type Listener = () => void;

export interface ConversationSendScope {
  hostId: string | null;
  workspaceId: string | undefined;
  sessionId: string | undefined;
}

interface PendingConversationSend {
  readonly id: string;
  readonly entry: PatchTypeWithKey;
  readonly text: string;
  readonly executorConfig: ExecutorConfig | null;
  readonly baselineRunIds: ReadonlySet<string>;
  readonly agentRunId: string | null;
  readonly phase: 'sending' | 'accepted';
  readonly restoreDraft: boolean;
}

interface PendingConversationSendSnapshot {
  readonly sends: readonly PendingConversationSend[];
  readonly error: string | null;
  readonly failedDraft: {
    readonly id: string;
    readonly text: string;
    readonly executorConfig: ExecutorConfig | null;
  } | null;
}

export interface ConversationSendHandle {
  readonly id: string;
  scopeKey: string;
}

const EMPTY: PendingConversationSendSnapshot = {
  sends: [],
  error: null,
  failedDraft: null,
};

// In-flight presentation only. These records never enter the canonical store,
// RunState, native history, or the durable message queue.
const snapshots = new Map<string, PendingConversationSendSnapshot>();
const listeners = new Map<string, Set<Listener>>();
let nextSendId = 0;

function scopeKey(scope: ConversationSendScope) {
  return JSON.stringify([
    scope.hostId,
    scope.workspaceId ?? null,
    scope.sessionId ?? null,
  ]);
}

function publish(key: string, snapshot: PendingConversationSendSnapshot) {
  if (!snapshot.sends.length && !snapshot.error && !snapshot.failedDraft)
    snapshots.delete(key);
  else snapshots.set(key, snapshot);
  listeners.get(key)?.forEach((listener) => listener());
}

export function beginConversationSend({
  text,
  baselineRunIds = [],
  executorConfig = null,
  ...scope
}: ConversationSendScope & {
  text: string;
  baselineRunIds?: Iterable<string>;
  executorConfig?: ExecutorConfig | null;
}): ConversationSendHandle | null {
  const key = scopeKey(scope);
  const current = snapshots.get(key) ?? EMPTY;
  if (
    !scope.workspaceId ||
    current.sends.some((send) => send.phase === 'sending')
  )
    return null;
  const id = String(++nextSendId);
  publish(key, {
    sends: [
      ...current.sends,
      {
        id,
        text,
        executorConfig,
        baselineRunIds: new Set(baselineRunIds),
        agentRunId: null,
        phase: 'sending',
        restoreDraft: true,
        entry: {
          type: 'NORMALIZED_ENTRY',
          patchKey: `pending-send:${id}`,
          content: {
            entry_type: { type: 'user_message' },
            content: text,
            timestamp: null,
          },
        },
      },
    ],
    error: null,
    failedDraft: null,
  });
  return { id, scopeKey: key };
}

/** Bind before selecting the durable session, so remounts see the same send. */
export function bindConversationSend(
  handle: ConversationSendHandle,
  scope: ConversationSendScope
) {
  const snapshot = snapshots.get(handle.scopeKey);
  const send = snapshot?.sends.find((send) => send.id === handle.id);
  if (!send || !snapshot) return;
  const nextKey = scopeKey(scope);
  if (nextKey === handle.scopeKey) return;
  const previousKey = handle.scopeKey;
  handle.scopeKey = nextKey;
  const target = snapshots.get(nextKey) ?? EMPTY;
  publish(nextKey, {
    ...target,
    sends: [...target.sends, send],
    error: null,
    failedDraft: null,
  });
  publish(previousKey, {
    ...snapshot,
    sends: snapshot.sends.filter((item) => item.id !== handle.id),
  });
}

export function acceptConversationSend(
  handle: ConversationSendHandle,
  agentRunId: string
) {
  const snapshot = snapshots.get(handle.scopeKey);
  if (!snapshot?.sends.some((send) => send.id === handle.id)) return;
  publish(handle.scopeKey, {
    ...snapshot,
    sends: snapshot.sends.map((send) =>
      send.id === handle.id ? { ...send, phase: 'accepted', agentRunId } : send
    ),
  });
}

export function failConversationSend(
  handle: ConversationSendHandle,
  error: string
) {
  const snapshot = snapshots.get(handle.scopeKey);
  const send = snapshot?.sends.find((send) => send.id === handle.id);
  if (!send || !snapshot) return;
  publish(handle.scopeKey, {
    sends: snapshot.sends.filter((item) => item.id !== handle.id),
    error,
    failedDraft: send.restoreDraft
      ? { id: send.id, text: send.text, executorConfig: send.executorConfig }
      : null,
  });
}

/** User edits remain newer intent even after close/reopen or typing then clearing. */
export function markConversationDraftEdited(scope: ConversationSendScope) {
  const key = scopeKey(scope);
  const snapshot = snapshots.get(key);
  if (
    !snapshot ||
    (!snapshot.failedDraft && !snapshot.sends.some((send) => send.restoreDraft))
  )
    return;
  publish(key, {
    ...snapshot,
    sends: snapshot.sends.map((send) =>
      send.restoreDraft ? { ...send, restoreDraft: false } : send
    ),
    failedDraft: null,
  });
}

export function clearConversationSendError(scope: ConversationSendScope) {
  const key = scopeKey(scope);
  const snapshot = snapshots.get(key);
  if (snapshot?.error) publish(key, { ...snapshot, error: null });
}

export function consumeFailedConversationDraft(scope: ConversationSendScope) {
  const key = scopeKey(scope);
  const snapshot = snapshots.get(key);
  if (snapshot?.failedDraft) publish(key, { ...snapshot, failedDraft: null });
}

export function settleConversationSend(
  scope: ConversationSendScope,
  entries: readonly PatchTypeWithKey[]
) {
  const key = scopeKey(scope);
  const snapshot = snapshots.get(key);
  if (!snapshot?.sends.length) return;
  const canonicalUserRunIds = new Set(
    entries.flatMap((entry) =>
      entry.canonical &&
      entry.type === 'NORMALIZED_ENTRY' &&
      entry.content.entry_type.type === 'user_message'
        ? [entry.canonical.agentRunId]
        : []
    )
  );
  const sends = snapshot.sends.filter(
    (send) => !send.agentRunId || !canonicalUserRunIds.has(send.agentRunId)
  );
  if (sends.length !== snapshot.sends.length)
    publish(key, { ...snapshot, sends });
}

export function projectPendingConversationSend(
  entries: PatchTypeWithKey[],
  snapshot: PendingConversationSendSnapshot
): PatchTypeWithKey[] {
  if (!snapshot.sends.length) return entries;
  const sending = snapshot.sends.find((send) => send.phase === 'sending');

  // The HTTP acknowledgement owns request/run correlation. Until it arrives,
  // buffer only newly introduced canonical runs in this presentation, avoiding
  // duplicate echoes even when the stream wins the race. Existing history and
  // the canonical store/runtime controls continue to update normally.
  let projected = sending
    ? entries.filter(
        (entry) =>
          !entry.canonical ||
          sending.baselineRunIds.has(entry.canonical.agentRunId)
      )
    : entries;
  for (const pending of snapshot.sends) {
    if (
      projected.some(
        (entry) =>
          entry.canonical?.agentRunId === pending.agentRunId &&
          entry.type === 'NORMALIZED_ENTRY' &&
          entry.content.entry_type.type === 'user_message'
      )
    )
      continue;
    const firstRunEntry = pending.agentRunId
      ? projected.findIndex(
          (entry) => entry.canonical?.agentRunId === pending.agentRunId
        )
      : -1;
    projected =
      firstRunEntry < 0
        ? [...projected, pending.entry]
        : [
            ...projected.slice(0, firstRunEntry),
            pending.entry,
            ...projected.slice(firstRunEntry),
          ];
  }
  return projected;
}

export function usePendingConversationSend(scope: ConversationSendScope) {
  const key = scopeKey(scope);
  const subscribe = useCallback(
    (listener: Listener) => {
      let scoped = listeners.get(key);
      if (!scoped) {
        scoped = new Set();
        listeners.set(key, scoped);
      }
      scoped.add(listener);
      return () => {
        scoped.delete(listener);
        if (!scoped.size) listeners.delete(key);
      };
    },
    [key]
  );
  const getSnapshot = useCallback(() => snapshots.get(key) ?? EMPTY, [key]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
