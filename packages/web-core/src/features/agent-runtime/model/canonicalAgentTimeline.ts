import type {
  AgentEventCursor,
  AgentEventEnvelope,
  AgentEventPayload,
  AgentRunStatus,
  RunState,
} from 'shared/types';

export type CanonicalAgentTimelineItemKind =
  | 'message'
  | 'thinking'
  | 'tool'
  | 'approval'
  | 'input'
  | 'usage'
  | 'status'
  | 'session'
  | 'error'
  | 'degraded'
  | 'extension'
  | 'unknown';

export interface CanonicalAgentTimelineItem {
  readonly eventId: string;
  readonly runAttemptNumber: number;
  readonly sequence: bigint;
  readonly timestamp: string;
  readonly kind: CanonicalAgentTimelineItemKind;
  readonly content: string | null;
  readonly status: AgentRunStatus | null;
  readonly payload: AgentEventEnvelope['payload'];
}

export interface CanonicalAgentTimeline {
  readonly state: RunState | null;
  readonly events: readonly AgentEventEnvelope[];
  readonly items: readonly CanonicalAgentTimelineItem[];
  readonly cursor: AgentEventCursor | null;
}

export const emptyCanonicalAgentTimeline = (): CanonicalAgentTimeline => ({
  state: null,
  events: [],
  items: [],
  cursor: null,
});

function sequenceBigInt(sequence: number | bigint): bigint {
  return typeof sequence === 'bigint' ? sequence : BigInt(sequence);
}

function eventSort(a: AgentEventEnvelope, b: AgentEventEnvelope): number {
  return (
    a.run_attempt_number - b.run_attempt_number ||
    (sequenceBigInt(a.sequence) < sequenceBigInt(b.sequence)
      ? -1
      : sequenceBigInt(a.sequence) > sequenceBigInt(b.sequence)
        ? 1
        : 0)
  );
}

function assertNeverPayload(payload: never): never {
  throw new Error(
    `Unsupported canonical AgentEvent payload: ${String(payload)}`
  );
}

function itemFromEvent(event: AgentEventEnvelope): CanonicalAgentTimelineItem {
  const payload = event.payload;
  let kind: CanonicalAgentTimelineItemKind;
  let content: string | null = null;
  let status: AgentRunStatus | null = null;

  switch (payload.type) {
    case 'conversation':
      // Legacy run diagnostics keep the typed payload available. The session
      // read model owns v2 content, ordering and streaming replacements.
      kind = 'extension';
      break;
    case 'message':
      kind = 'message';
      content = payload.data.message.content;
      break;
    case 'thinking':
      kind = 'thinking';
      content = payload.data.content;
      break;
    case 'tool_call':
      kind = 'tool';
      content = payload.data.tool_name;
      break;
    case 'file_changes':
      kind = 'tool';
      content = payload.data.changes.map((change) => change.path).join('\n');
      break;
    case 'approval_requested':
    case 'approval_resolved':
      kind = 'approval';
      content =
        payload.type === 'approval_requested'
          ? payload.data.tool_name
          : (payload.data.reason ?? null);
      break;
    case 'input_requested':
    case 'input_resolved':
      kind = 'input';
      content = payload.type === 'input_requested' ? payload.data.prompt : null;
      break;
    case 'token_usage':
      kind = 'usage';
      break;
    case 'lifecycle_changed':
      kind = 'status';
      status = payload.data.status;
      break;
    case 'session_observed':
      kind = 'session';
      content = payload.data.provider_session.provider_session_id;
      break;
    case 'error':
      kind = 'error';
      content = payload.data.error.message;
      break;
    case 'projection_degraded':
      kind = 'degraded';
      content = payload.data.reason;
      break;
    case 'provider_extension':
      kind = 'extension';
      content = `${payload.data.provider_namespace}:${payload.data.provider_event}`;
      break;
    case 'unknown':
      kind = 'unknown';
      content = payload.data.event_type;
      break;
    default:
      return assertNeverPayload(payload);
  }

  return {
    eventId: event.event_id,
    runAttemptNumber: event.run_attempt_number,
    sequence: sequenceBigInt(event.sequence),
    timestamp: event.timestamp,
    kind,
    content,
    status,
    payload,
  };
}

/**
 * Merge replay or live events into one deterministic canonical timeline.
 * Event payloads are already decoded by the provider adapter; this reducer
 * never reads provider logs, native audit frames, or legacy process records.
 */
export function mergeCanonicalAgentTimeline(
  previous: CanonicalAgentTimeline,
  events: readonly AgentEventEnvelope[],
  state?: RunState | null,
  serverCursor?: AgentEventCursor | null
): CanonicalAgentTimeline {
  const knownIds = new Set(
    events.length ? previous.events.map((event) => event.event_id) : []
  );
  const freshEvents = events.filter((event) => {
    if (knownIds.has(event.event_id)) return false;
    knownIds.add(event.event_id);
    return true;
  });
  if (
    freshEvents.some(
      (event, index) =>
        index > 0 && eventSort(freshEvents[index - 1], event) > 0
    )
  ) {
    freshEvents.sort(eventSort);
  }
  if (
    freshEvents.length === 0 &&
    state === undefined &&
    serverCursor === undefined
  ) {
    return previous;
  }

  // Live traffic is ordered. Preserve existing item identities instead of
  // sorting and projecting the complete history for every text delta. Replays
  // arriving out of order use the deterministic fallback without losing data.
  const lastEvent = previous.events.at(-1);
  const appendOnly =
    !lastEvent ||
    !freshEvents.length ||
    eventSort(lastEvent, freshEvents[0]) <= 0;
  const mergedEvents =
    freshEvents.length === 0
      ? previous.events
      : appendOnly
        ? [...previous.events, ...freshEvents]
        : [...previous.events, ...freshEvents].sort(eventSort);
  const mergedItems =
    freshEvents.length === 0
      ? previous.items
      : appendOnly
        ? [...previous.items, ...freshEvents.map(itemFromEvent)]
        : (() => {
            const existing = new Map(
              previous.items.map((item) => [item.eventId, item])
            );
            return mergedEvents.map(
              (event) => existing.get(event.event_id) ?? itemFromEvent(event)
            );
          })();
  const latestEvent = mergedEvents.at(-1);
  const eventCursor = latestEvent
    ? ({
        run_attempt_number: latestEvent.run_attempt_number,
        sequence: sequenceBigInt(latestEvent.sequence),
      } satisfies AgentEventCursor)
    : previous.cursor;
  const cursor = newestCursor(
    newestCursor(previous.cursor, eventCursor),
    serverCursor
  );

  return {
    state:
      state === undefined
        ? previous.state
        : newestRunState(previous.state, state),
    events: mergedEvents,
    items: mergedItems,
    cursor,
  };
}

function newestCursor(
  current: AgentEventCursor | null,
  candidate: AgentEventCursor | null | undefined
): AgentEventCursor | null {
  if (candidate === undefined || candidate === null) return current;
  if (!current) return candidate;
  if (candidate.run_attempt_number !== current.run_attempt_number) {
    return candidate.run_attempt_number > current.run_attempt_number
      ? candidate
      : current;
  }
  return sequenceBigInt(candidate.sequence) >= sequenceBigInt(current.sequence)
    ? candidate
    : current;
}

function newestRunState(
  current: RunState | null,
  candidate: RunState | null
): RunState | null {
  if (!candidate || !current) return candidate ?? current;
  if (candidate.last_run_attempt_number !== current.last_run_attempt_number) {
    return candidate.last_run_attempt_number > current.last_run_attempt_number
      ? candidate
      : current;
  }
  const candidateSequence = sequenceBigInt(candidate.last_event_sequence);
  const currentSequence = sequenceBigInt(current.last_event_sequence);
  if (candidateSequence !== currentSequence) {
    return candidateSequence > currentSequence ? candidate : current;
  }
  return candidate.updated_at >= current.updated_at ? candidate : current;
}

export function isCanonicalProjectionAvailable(
  state: RunState | null
): boolean {
  return state?.projection_status === 'current';
}

export function isCanonicalToolEventActive(
  payload: AgentEventPayload
): boolean {
  if (payload.type !== 'tool_call') return false;
  switch (payload.data.status) {
    case 'created':
    case 'running':
    case 'waiting_approval':
      return true;
    case 'approved':
    case 'denied':
    case 'succeeded':
    case 'failed':
    case 'timed_out':
      return false;
    default:
      return false;
  }
}

export function isCanonicalAgentRunTerminal(state: RunState | null): boolean {
  if (!state) return false;
  return (
    state.status === 'succeeded' ||
    state.status === 'failed' ||
    state.status === 'cancelled' ||
    state.status === 'crashed' ||
    state.status === 'audit_failed'
  );
}
