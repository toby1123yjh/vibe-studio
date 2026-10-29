import type {
  AgentRunPortSnapshot,
  AgentRunSummary,
  ConversationBlock,
  ConversationChangesPage,
  ConversationCursor,
  ConversationPage,
  ConversationStreamMessage,
  NormalizedEntry,
} from 'shared/types';
import type {
  CanonicalConversationProjection,
  PatchTypeWithKey,
} from '@/shared/hooks/useConversationHistory/types';
import type { agentConversationApi } from '@/shared/lib/agentConversationApi';
import { newestRunState } from './canonicalAgentTimeline';
import {
  buildCanonicalAgentSessionTimeline,
  isCanonicalRunActive,
  type CanonicalAgentSessionTimeline,
} from './canonicalAgentSessionTimeline';

type Listener = () => void;
export type AgentConversationTransport = Pick<
  typeof agentConversationApi,
  'page' | 'changes' | 'openStream' | 'detail'
>;
const MAX_WINDOW_BLOCKS = 500;

export interface ConversationSessionSnapshot {
  readonly timeline: CanonicalAgentSessionTimeline | null;
  readonly conversation: CanonicalConversationProjection;
  readonly pendingInteractions: readonly ConversationBlock[];
  readonly isLoading: boolean;
  readonly isConnected: boolean;
  readonly error: string | null;
  readonly hasMore: boolean;
  readonly hasNewer: boolean;
  readonly isLoadingHistory: boolean;
}

function compareRevision(a: string, b: string) {
  const left = BigInt(a);
  const right = BigInt(b);
  return left < right ? -1 : left > right ? 1 : 0;
}

function runSignature(runs: readonly AgentRunSummary[]) {
  // Event cursor/timestamp changes are not composer changes. Keep their traffic
  // out of the header/model picker/input subscription.
  return JSON.stringify(
    runs.map(({ agent_run_id, created_at, state }) => [
      agent_run_id,
      created_at,
      state.status,
      state.projection_status,
      state.last_run_attempt_id,
      state.last_run_attempt_number,
      state.observed_configuration,
      state.last_error,
    ])
  );
}

/** A structural reference for the existing list/virtualizer, not a transcript.
 * Content/state rendering subscribes to the block itself. No native semantics
 * or delta inference belong here.
 */
function blockReference(
  block: ConversationBlock,
  runs: readonly AgentRunSummary[]
): PatchTypeWithKey {
  const run = runs.find(
    (candidate) => candidate.agent_run_id === block.agent_run_id
  );
  const active = !block.finished && isCanonicalRunActive(run?.state);
  let entryType: NormalizedEntry['entry_type'];
  switch (block.kind) {
    case 'text':
      entryType = {
        type:
          block.role === 'user'
            ? 'user_message'
            : block.role === 'system'
              ? 'system_message'
              : 'assistant_message',
      };
      break;
    case 'reasoning':
      entryType = { type: 'thinking' };
      break;
    case 'tool':
      entryType = {
        type: 'tool_use',
        tool_name: block.fields.tool_name ?? '',
        action_type: {
          action: 'tool',
          tool_name: block.fields.tool_name ?? '',
          arguments: null,
          result: null,
        },
        // This is only list structure. The block renderer uses native status;
        // absence of a result must never be manufactured into success.
        status: {
          status: active
            ? 'created'
            : block.fields.status === 'succeeded'
              ? 'success'
              : block.fields.status === 'failed'
                ? 'failed'
                : 'created',
        },
      };
      break;
    default:
      entryType = { type: 'system_message' };
  }
  return {
    type: 'NORMALIZED_ENTRY',
    patchKey: `conversation-block:${block.block_id}`,
    conversationBlockId: block.block_id,
    conversationParentBlockId: block.parent_block_id,
    content: {
      entry_type: entryType,
      // User messages are immutable after submission and supply turn navigation.
      // Other content is read from the per-block subscription.
      content: block.role === 'user' ? (block.fields.text ?? '') : '',
      timestamp: block.created_at,
    },
    canonical: {
      agentRunId: block.agent_run_id,
      runAttemptId: block.run_attempt_id,
      runAttemptNumber:
        block.run_attempt_id === run?.state.last_run_attempt_id
          ? (run.state.last_run_attempt_number ?? null)
          : null,
      eventId: block.source_event_id,
      eventIds: [block.source_event_id],
      sequence: 0n,
      active,
    },
  };
}

function referenceSignature(
  block: ConversationBlock,
  runs: readonly AgentRunSummary[]
) {
  const run = runs.find((item) => item.agent_run_id === block.agent_run_id);
  return JSON.stringify([
    block.kind,
    block.role,
    block.parent_block_id,
    block.fields.tool_name,
    block.fields.status,
    !block.finished && isCanonicalRunActive(run?.state),
    block.role === 'user' ? block.fields.text : null,
  ]);
}

/** Session-scoped read cache. The server alone merges conversation facts. */
export class AgentConversationStore {
  private readonly blocks = new Map<string, ConversationBlock>();
  private readonly references = new Map<
    string,
    { signature: string; entry: PatchTypeWithKey }
  >();
  private readonly blockListeners = new Map<string, Set<Listener>>();
  private readonly listeners = new Set<Listener>();
  private readonly changedBlockIds = new Set<string>();
  private ids: string[] = [];
  private entries: PatchTypeWithKey[] = [];
  private runs: AgentRunSummary[] = [];
  private readonly acknowledgedRuns = new Map<string, AgentRunSummary>();
  private runsKey = '';
  private pending: ConversationBlock[] = [];
  private pendingKey = '';
  private cursor: ConversationCursor | null = null;
  private metadataRevision: string | null = null;
  private before: string | null = null;
  private windowAtTail = true;
  private generation = 0;
  private controller: AbortController | null = null;
  private socket: WebSocket | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private retryAttempt = 0;
  private refreshPromise: Promise<void> | null = null;
  private readonly recoveringBlocks = new Map<
    string,
    { revision: string; field: string }
  >();
  private timeline: CanonicalAgentSessionTimeline | null = null;
  private snapshot: ConversationSessionSnapshot;

  constructor(
    readonly sessionId: string,
    readonly hostId: string | null,
    private readonly transport?: AgentConversationTransport
  ) {
    this.snapshot = {
      timeline: null,
      conversation: {
        entries: [],
        activeAgentRunIds: new Set(),
        runCount: 0,
        isLoading: Boolean(sessionId),
        isRunning: false,
        projectionDegraded: false,
        latestStatus: null,
      },
      pendingInteractions: [],
      isLoading: Boolean(sessionId),
      isConnected: false,
      error: null,
      hasMore: false,
      hasNewer: false,
      isLoadingHistory: false,
    };
  }

  getSnapshot = () => this.snapshot;
  getBlock = (id: string) => this.blocks.get(id);
  getCursor = () => this.cursor;

  applyRunSnapshot = ({ agent_run_id, state }: AgentRunPortSnapshot) => {
    if (
      state.session_id !== this.sessionId ||
      state.agent_run_id !== agent_run_id
    )
      return;
    const current = this.runs.find((run) => run.agent_run_id === agent_run_id);
    // The stream may already have delivered a newer or terminal state while
    // the follow-up response was in flight. Existing state wins exact ties.
    if (current && newestRunState(state, current.state) === current.state)
      return;
    this.acknowledgedRuns.set(agent_run_id, {
      agent_run_id,
      session_id: state.session_id,
      turn_id: state.turn_id,
      // Follow-up returns RunState without creation metadata. Use its server
      // timestamp only until conversation discovery supplies the real summary.
      created_at: current?.created_at ?? state.updated_at,
      updated_at: state.updated_at,
      state,
    });
    this.reconcile(this.runs, this.pending);
    this.publish();
  };

  subscribe = (listener: Listener) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  subscribeBlock = (id: string, listener: Listener) => {
    let set = this.blockListeners.get(id);
    if (!set) {
      set = new Set();
      this.blockListeners.set(id, set);
    }
    set.add(listener);
    return () => {
      set.delete(listener);
      if (!set.size) this.blockListeners.delete(id);
    };
  };

  private notify(sessionChanged = false) {
    // A group may subscribe the same callback to hundreds of blocks. Notify it
    // once after the entire page and its metadata are committed, rather than
    // rescanning the group for every block or exposing a partially applied page.
    const listeners = new Set<Listener>(sessionChanged ? this.listeners : []);
    for (const id of this.changedBlockIds)
      this.blockListeners
        .get(id)
        ?.forEach((listener) => listeners.add(listener));
    this.changedBlockIds.clear();
    listeners.forEach((listener) => listener());
  }

  private publish(status: Partial<ConversationSessionSnapshot> = {}) {
    const previous = this.snapshot;
    const timeline = this.timeline;
    const conversation = previous.conversation;
    const isLoading = status.isLoading ?? previous.isLoading;
    const changed =
      this.entries !== conversation.entries ||
      timeline !== previous.timeline ||
      isLoading !== previous.isLoading;
    const next = {
      ...previous,
      ...status,
      timeline,
      pendingInteractions: this.pending,
      conversation: changed
        ? {
            entries: this.entries,
            activeAgentRunIds: new Set(
              (timeline?.runs ?? [])
                .filter((run) => isCanonicalRunActive(run.summary.state))
                .map((run) => run.summary.agent_run_id)
            ),
            runCount: this.runs.length,
            isLoading,
            isRunning: timeline?.isRunning ?? false,
            projectionDegraded: timeline?.projectionDegraded ?? false,
            latestStatus: timeline?.latestRun?.summary.state.status ?? null,
          }
        : conversation,
    };
    const sessionChanged = (
      Object.keys(next) as Array<keyof ConversationSessionSnapshot>
    ).some((key) => next[key] !== previous[key]);
    if (sessionChanged) this.snapshot = next;
    this.notify(sessionChanged);
  }

  private ingest(blocks: readonly ConversationBlock[], addToWindow: boolean) {
    let orderChanged = false;
    let hasNewer = false;
    for (const block of blocks) {
      const previous = this.blocks.get(block.block_id);
      // A full baseline at the same revision replaces an optional append fast
      // path, including finish/metadata fields that were not in the delta.
      if (
        previous &&
        (compareRevision(block.revision, previous.revision) < 0 ||
          JSON.stringify(block) === JSON.stringify(previous))
      )
        continue;
      const isNew = !this.ids.includes(block.block_id);
      const latestPosition = this.blocks.get(this.ids.at(-1) ?? '')?.position;
      // While reading an evicted historical window, retain the reader's window.
      // New tail entries remain on the server until returning to latest.
      if (
        isNew &&
        !addToWindow &&
        !this.windowAtTail &&
        latestPosition &&
        block.position > latestPosition
      ) {
        hasNewer = true;
        continue;
      }
      this.blocks.set(block.block_id, block);
      if (isNew && addToWindow) {
        this.ids.push(block.block_id);
        orderChanged = true;
      } else if (isNew && this.windowAtTail) {
        this.ids.push(block.block_id);
        orderChanged = true;
      }
      this.changedBlockIds.add(block.block_id);
    }
    if (orderChanged)
      this.ids.sort((a, b) => {
        const left = this.blocks.get(a)!;
        const right = this.blocks.get(b)!;
        return left.position < right.position
          ? -1
          : left.position > right.position
            ? 1
            : a.localeCompare(b);
      });
    return hasNewer;
  }

  private reconcile(
    runs: AgentRunSummary[],
    pending: ConversationBlock[],
    revision?: string
  ) {
    // Backward pages can observe newer run/request metadata without covering
    // live changes. Keep that metadata's version independent of the recovery
    // cursor so the subsequent catch-up pages cannot make controls regress.
    const acceptsMetadata =
      revision !== undefined &&
      (!this.metadataRevision ||
        compareRevision(revision, this.metadataRevision) >= 0);
    if (
      revision &&
      this.metadataRevision &&
      compareRevision(revision, this.metadataRevision) < 0
    ) {
      runs = this.runs;
      pending = this.pending;
    } else if (revision) {
      this.metadataRevision = revision;
    }
    if (this.acknowledgedRuns.size) {
      const discovered = new Set<string>();
      runs = runs.map((run) => {
        const acknowledged = this.acknowledgedRuns.get(run.agent_run_id);
        if (!acknowledged) return run;
        discovered.add(run.agent_run_id);
        const state =
          newestRunState(acknowledged.state, run.state) ?? run.state;
        if (state === run.state) {
          // Only actual discovery can finish the handoff. Reconciliation with
          // our own cache (or a stale page) is not evidence that it caught up.
          if (acceptsMetadata) this.acknowledgedRuns.delete(run.agent_run_id);
          return run;
        }
        const merged = { ...run, state, updated_at: state.updated_at };
        this.acknowledgedRuns.set(run.agent_run_id, merged);
        return merged;
      });
      for (const [id, run] of this.acknowledgedRuns) {
        if (!discovered.has(id)) runs.push(run);
      }
    }
    const key = runSignature(runs);
    const previousRuns = this.runs;
    // Keep version facts current even when token-only updates intentionally
    // preserve the public timeline reference and composer subscription.
    this.runs = runs;
    if (key !== this.runsKey) {
      this.runsKey = key;
      this.timeline = buildCanonicalAgentSessionTimeline(this.sessionId, runs);
      for (const [id, block] of this.blocks) {
        const before = previousRuns.find(
          (run) => run.agent_run_id === block.agent_run_id
        );
        const after = runs.find(
          (run) => run.agent_run_id === block.agent_run_id
        );
        if (
          isCanonicalRunActive(before?.state) !==
          isCanonicalRunActive(after?.state)
        ) {
          this.blocks.set(id, { ...block });
          this.changedBlockIds.add(id);
        }
      }
    }
    const pendingKey = JSON.stringify(
      pending.map((block) => [block.block_id, block.fields.interaction])
    );
    if (pendingKey !== this.pendingKey) {
      this.pendingKey = pendingKey;
      this.pending = pending;
    }
    // Pending requests outside the window still have an addressable baseline.
    for (const block of pending) {
      const previous = this.blocks.get(block.block_id);
      if (!previous || compareRevision(block.revision, previous.revision) > 0) {
        this.blocks.set(block.block_id, block);
        this.changedBlockIds.add(block.block_id);
      }
    }
    const retained = new Set([
      ...this.ids,
      ...pending.map((block) => block.block_id),
    ]);
    for (const id of this.blocks.keys())
      if (!retained.has(id)) {
        this.blocks.delete(id);
        this.references.delete(id);
        this.changedBlockIds.add(id);
      }
    const next = this.ids.flatMap((id) => {
      const block = this.blocks.get(id);
      if (!block) return [];
      const signature = referenceSignature(block, this.runs);
      let reference = this.references.get(id);
      if (!reference || reference.signature !== signature) {
        reference = { signature, entry: blockReference(block, this.runs) };
        this.references.set(id, reference);
      }
      return [reference.entry];
    });
    if (
      next.length !== this.entries.length ||
      next.some((entry, index) => entry !== this.entries[index])
    )
      this.entries = next;
  }

  applyPage = (page: ConversationPage, replace = false) => {
    if (page.session_id !== this.sessionId) return;
    const epochChanged = this.cursor?.epoch !== page.cursor.epoch;
    if (replace || epochChanged) {
      const oldIds = [...this.blocks.keys()];
      this.blocks.clear();
      this.references.clear();
      this.ids = [];
      this.windowAtTail = true;
      this.cursor = page.cursor;
      if (epochChanged) this.metadataRevision = null;
      for (const id of oldIds) this.changedBlockIds.add(id);
    }
    // A backward page may race a newer live snapshot; retain newer blocks.
    this.ingest(page.blocks, true);
    // Backward pagination does not cover live changes since the prior cursor.
    // Advancing to its newer DB revision would silently skip those changes.
    this.before = page.previous_cursor;
    const evictedTail = this.ids.length > MAX_WINDOW_BLOCKS && !replace;
    if (this.ids.length > MAX_WINDOW_BLOCKS)
      this.ids = evictedTail
        ? this.ids.slice(0, MAX_WINDOW_BLOCKS)
        : this.ids.slice(-MAX_WINDOW_BLOCKS);
    if (evictedTail) this.windowAtTail = false;
    this.reconcile(page.runs, page.pending_interactions, page.cursor.revision);
    this.publish({
      isLoading: false,
      isLoadingHistory: false,
      error: null,
      hasMore: page.has_more,
      hasNewer: evictedTail || (!replace && this.snapshot.hasNewer),
    });
  };

  applyChanges = (page: ConversationChangesPage): boolean => {
    if (
      page.session_id !== this.sessionId ||
      page.reset_required ||
      !this.cursor ||
      page.cursor.epoch !== this.cursor.epoch
    )
      return false;
    if (compareRevision(page.cursor.revision, this.cursor.revision) < 0)
      return true;
    const hasNewer = this.ingest(page.blocks, false);
    for (const id of page.tombstones) {
      this.blocks.delete(id);
      this.references.delete(id);
      this.ids = this.ids.filter((candidate) => candidate !== id);
      this.changedBlockIds.add(id);
    }
    if (this.ids.length > MAX_WINDOW_BLOCKS) {
      this.ids = this.ids.slice(-MAX_WINDOW_BLOCKS);
      this.before = this.blocks.get(this.ids[0])?.position ?? this.before;
    }
    this.cursor = page.cursor;
    this.reconcile(page.runs, page.pending_interactions, page.cursor.revision);
    this.publish({
      error: null,
      hasMore: this.snapshot.hasMore || this.ids.length === MAX_WINDOW_BLOCKS,
      hasNewer: hasNewer || this.snapshot.hasNewer,
    });
    return true;
  };

  applyAppend = (
    data: Extract<ConversationStreamMessage, { type: 'append' }>['data']
  ) => {
    if (data.session_id !== this.sessionId || data.epoch !== this.cursor?.epoch)
      return;
    for (const append of data.appends) {
      const block = this.blocks.get(append.block_id);
      if (!block || block.revision !== append.base_revision) {
        if (!block || compareRevision(block.revision, append.revision) < 0)
          this.recoverBlock(append.block_id, append.field, append.revision);
        continue;
      }
      const field = append.field;
      if (
        field !== 'text' &&
        field !== 'output_text' &&
        field !== 'raw_arguments'
      )
        continue;
      const content = block.fields[field] ?? '';
      const bytes = new TextEncoder().encode(content).byteLength;
      const body = block.bodies.find((item) => item.field === field);
      // A truncated head is not the suffix to which a native delta belongs.
      if (
        (body && Number(body.byte_length) > bytes) ||
        bytes + new TextEncoder().encode(append.text).byteLength > 32768
      ) {
        // Keep the bounded head plus exact body length. It is already a full
        // head, so appending to it would put native suffixes at a false offset.
        // The visible body follows a bounded detail tail at a lower cadence.
        this.blocks.set(block.block_id, {
          ...block,
          revision: append.revision,
          bodies: block.bodies.map((item) =>
            item.field === field
              ? {
                  ...item,
                  byte_length:
                    Number(item.byte_length) +
                    new TextEncoder().encode(append.text).byteLength,
                }
              : item
          ),
        });
        this.changedBlockIds.add(block.block_id);
        continue;
      }
      this.blocks.set(block.block_id, {
        ...block,
        revision: append.revision,
        fields: { ...block.fields, [field]: content + append.text },
        bodies: block.bodies.map((item) =>
          item.field === field
            ? {
                ...item,
                byte_length:
                  Number(item.byte_length) +
                  new TextEncoder().encode(append.text).byteLength,
              }
            : item
        ),
      });
      this.changedBlockIds.add(block.block_id);
    }
    // Only a committed changes page advances the session recovery cursor.
    this.notify();
  };

  private recoverBlock(id: string, field: string, revision: string) {
    if (!this.transport || (!this.windowAtTail && !this.blocks.has(id))) return;
    const pending = this.recoveringBlocks.get(id);
    if (pending) {
      if (compareRevision(revision, pending.revision) > 0)
        pending.revision = revision;
      return;
    }
    const target = { revision, field };
    this.recoveringBlocks.set(id, target);
    const generation = this.generation;
    void (async () => {
      while (generation === this.generation) {
        const detail = await this.transport!.detail(
          this.sessionId,
          this.hostId,
          id,
          target.field,
          0,
          this.controller?.signal
        );
        if (generation !== this.generation) return;
        const hasNewer = this.ingest([detail.block], false);
        this.reconcile(this.runs, this.pending);
        this.publish({ hasNewer: hasNewer || this.snapshot.hasNewer });
        if (compareRevision(detail.block.revision, target.revision) >= 0)
          return;
      }
    })()
      .catch((error: unknown) => {
        if (generation === this.generation)
          this.publish({
            error: error instanceof Error ? error.message : String(error),
          });
      })
      .finally(() => {
        if (this.recoveringBlocks.get(id) === target)
          this.recoveringBlocks.delete(id);
      });
  }

  /** Detail reads carry a fresh bounded baseline, but do not cover session
   * changes and therefore cannot advance the recovery cursor. */
  acceptDetailBlock = (block: ConversationBlock) => {
    if (!this.blocks.has(block.block_id)) return;
    const hasNewer = this.ingest([block], false);
    this.reconcile(this.runs, this.pending);
    this.publish({ hasNewer: hasNewer || this.snapshot.hasNewer });
  };

  loadPrevious = async () => {
    if (
      !this.snapshot.hasMore ||
      this.snapshot.isLoadingHistory ||
      !this.before
    )
      return;
    const generation = this.generation;
    const epoch = this.cursor?.epoch;
    this.publish({ isLoadingHistory: true });
    try {
      const page = await this.transport!.page(
        this.sessionId,
        this.hostId,
        this.before,
        this.controller?.signal
      );
      if (generation !== this.generation || epoch !== this.cursor?.epoch)
        return;
      this.applyPage(page);
    } catch (error) {
      if (generation === this.generation)
        this.publish({
          isLoadingHistory: false,
          error: error instanceof Error ? error.message : String(error),
        });
    }
  };

  refresh = (): Promise<void> => {
    if (this.refreshPromise) return this.refreshPromise;
    const generation = this.generation;
    this.refreshPromise = (async () => {
      if (!this.cursor) return;
      let hasMore = true;
      while (hasMore && generation === this.generation) {
        const page = await this.transport!.changes(
          this.sessionId,
          this.hostId,
          this.cursor!,
          this.controller?.signal
        );
        if (generation !== this.generation) return;
        if (!this.applyChanges(page)) {
          await this.loadLatest();
          return;
        }
        hasMore = page.has_more;
      }
    })().finally(() => {
      this.refreshPromise = null;
    });
    return this.refreshPromise;
  };

  loadLatest = async () => {
    const generation = this.generation;
    const page = await this.transport!.page(
      this.sessionId,
      this.hostId,
      null,
      this.controller?.signal
    );
    if (generation !== this.generation) return;
    // Preserve updates received while the tail request was in flight. The
    // tail baseline owns its membership, newer cached versions own content.
    if (
      this.cursor?.epoch === page.cursor.epoch &&
      compareRevision(this.cursor.revision, page.cursor.revision) > 0
    ) {
      page.blocks = page.blocks.map((block) => {
        const cached = this.blocks.get(block.block_id);
        return cached && compareRevision(cached.revision, block.revision) > 0
          ? cached
          : block;
      });
      const last = page.blocks.at(-1)?.position;
      const tail = this.ids
        .map((id) => this.blocks.get(id)!)
        .filter((block) => last && block.position > last);
      page.blocks.push(...tail);
      page.cursor = this.cursor;
      page.runs = this.runs;
      page.pending_interactions = this.pending;
    }
    this.applyPage(page, true);
  };

  start = () => {
    if (this.controller || !this.sessionId || !this.transport) return;
    this.controller = new AbortController();
    const generation = ++this.generation;
    const current = () =>
      generation === this.generation && !this.controller?.signal.aborted;
    const schedule = () => {
      if (!current() || this.retryTimer) return;
      this.retryTimer = setTimeout(
        () => {
          this.retryTimer = null;
          void connect();
        },
        Math.min(8000, 500 * 2 ** this.retryAttempt++)
      );
    };
    const connect = async () => {
      try {
        if (!this.cursor) await this.loadLatest();
        if (!current() || !this.cursor) return;
        const socket = await this.transport!.openStream(
          this.sessionId,
          this.hostId,
          this.cursor
        );
        if (!current()) {
          socket.close();
          return;
        }
        this.socket = socket;
        socket.onopen = () => {
          if (!current() || this.socket !== socket) return;
          this.retryAttempt = 0;
          this.publish({ isConnected: true, error: null });
        };
        socket.onmessage = (event) => {
          if (!current() || this.socket !== socket) return;
          try {
            const message = JSON.parse(event.data) as ConversationStreamMessage;
            switch (message.type) {
              case 'snapshot':
                this.applyPage(message.data, true);
                break;
              case 'append':
                this.applyAppend(message.data);
                break;
              case 'changes':
                if (!this.applyChanges(message.data)) {
                  socket.close();
                  void this.loadLatest()
                    .then(() => {
                      if (current()) schedule();
                    })
                    .catch(fail);
                }
                break;
              case 'ready':
                this.publish({
                  isLoading: false,
                  isConnected: true,
                  error: null,
                });
                break;
              case 'error':
                this.publish({ error: message.data.message });
                break;
            }
          } catch (error) {
            fail(error);
          }
        };
        socket.onerror = () => {
          if (current())
            this.publish({ error: 'Conversation connection interrupted' });
        };
        socket.onclose = () => {
          if (!current() || this.socket !== socket) return;
          this.socket = null;
          this.publish({ isConnected: false });
          schedule();
        };
      } catch (error) {
        fail(error);
        schedule();
      }
    };
    const fail = (error: unknown) => {
      if (current())
        this.publish({
          isLoading: false,
          error: error instanceof Error ? error.message : String(error),
        });
    };
    void connect();
  };

  stop = () => {
    ++this.generation;
    this.recoveringBlocks.clear();
    this.controller?.abort();
    this.controller = null;
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    const socket = this.socket;
    this.socket = null;
    socket?.close();
    this.publish({ isConnected: false });
  };
}

const sessions = new Map<
  string,
  { store: AgentConversationStore; readers: number; released: number }
>();

export function getAgentConversationStore(
  sessionId: string,
  hostId: string | null,
  transport: AgentConversationTransport
) {
  const key = JSON.stringify([hostId, sessionId]);
  let entry = sessions.get(key);
  if (!entry) {
    entry = {
      store: new AgentConversationStore(sessionId, hostId, transport),
      readers: 0,
      released: 0,
    };
    sessions.set(key, entry);
  }
  return entry.store;
}

export function retainAgentConversationStore(store: AgentConversationStore) {
  const key = JSON.stringify([store.hostId, store.sessionId]);
  const entry = sessions.get(key)!;
  ++entry.readers;
  store.start();
  return () => {
    --entry.readers;
    entry.released = Date.now();
    queueMicrotask(() => {
      if (!entry.readers) store.stop();
      const inactive = [...sessions.entries()]
        .filter(([, item]) => !item.readers)
        .sort((a, b) => b[1].released - a[1].released);
      for (const [id] of inactive.slice(3)) sessions.delete(id);
    });
  };
}
