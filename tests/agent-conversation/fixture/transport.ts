import type { ConversationBlock, ConversationPage } from 'shared/types';
export const requests: Array<{
  path: string;
  options: Record<string, unknown>;
}> = [];
export const sockets: FixtureSocket[] = [];
let revision = 1;
export const bodies = new Map<string, string>();
export function block(
  id: string,
  kind = 'text',
  fields: Record<string, unknown> = { text: 'Initial response' }
): ConversationBlock {
  return {
    block_id: id,
    agent_run_id: 'run-a',
    run_attempt_id: 'attempt-a',
    kind,
    role: 'assistant',
    native_id: id,
    parent_block_id: null,
    position: id,
    revision: String(revision),
    fields,
    bodies: [],
    finished: false,
    finish_reason: null,
    source_event_id: 'event',
    created_at: '2026-10-20T00:00:00Z',
    updated_at: '2026-10-20T00:00:00Z',
  } as ConversationBlock;
}
export const state: ConversationPage = {
  session_id: 'session',
  cursor: { epoch: 'epoch', revision: '1' },
  blocks: [block('answer')],
  runs: [
    {
      agent_run_id: 'run-a',
      session_id: 'session',
      turn_id: 'turn',
      created_at: '2026-10-20T00:00:00Z',
      updated_at: '2026-10-20T00:00:00Z',
      state: {
        agent_run_id: 'run-a',
        session_id: 'session',
        status: 'running',
        projection_status: 'current',
        last_run_attempt_id: 'attempt-a',
        last_run_attempt_number: 1,
      },
    },
    {
      agent_run_id: 'run-b',
      session_id: 'session',
      turn_id: 'turn-b',
      created_at: '2026-10-20T00:01:00Z',
      state: {
        agent_run_id: 'run-b',
        session_id: 'session',
        status: 'succeeded',
        projection_status: 'current',
        last_run_attempt_id: 'attempt-b',
        last_run_attempt_number: 1,
      },
    },
  ] as ConversationPage['runs'],
  pending_interactions: [],
  previous_cursor: null,
  has_more: false,
};

const listFixture = new URLSearchParams(location.search).has('list');
if (listFixture) {
  state.blocks = Array.from({ length: 600 }, (_, index) => ({
    ...block(String(index + 1).padStart(5, '0'), 'text', {
      text: `Message ${index + 1}`,
    }),
    finished: true,
  }));
  state.has_more = true;
}
function snapshot(before: string | null = null): ConversationPage {
  if (!listFixture) return state;
  const available = state.blocks.filter(
    (item) => !before || item.position < before
  );
  const blocks = available.slice(-50);
  return {
    ...state,
    blocks,
    has_more: available.length > blocks.length,
    previous_cursor: blocks[0]?.position ?? null,
  };
}
export function widenWindow() {
  for (const socket of sockets)
    socket.emit({
      type: 'changes',
      data: {
        ...state,
        tombstones: [],
        reset_required: false,
        has_more: false,
      },
    });
}

export class FixtureSocket {
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  constructor(
    readonly endpoint: string,
    readonly transport: unknown
  ) {}
  close() {
    this.onclose?.();
  }
  emit(message: unknown) {
    this.onmessage?.({ data: JSON.stringify(message) });
  }
}
export async function openLocalApiWebSocket(
  endpoint: string,
  transport: unknown
) {
  const socket = new FixtureSocket(endpoint, transport);
  sockets.push(socket);
  setTimeout(() => socket.onopen?.(), 0);
  return socket;
}
export async function handleApiResponse(response: Response) {
  const value = await response.json();
  if (!response.ok) throw new Error(value.message ?? 'Request failed');
  return value.data;
}
export async function makeLocalApiRequest(
  path: string,
  options: Record<string, unknown> = {}
) {
  requests.push({ path, options });
  if (path.endsWith('/interaction')) {
    if (new URLSearchParams(location.search).has('attempt-rollover')) {
      // The server can start another attempt before the old UI sees its update.
      // A captured reply must keep the original block's attempt identity.
      state.runs[0].state.last_run_attempt_id = 'attempt-restarted';
    }
    if (new URLSearchParams(location.search).has('uncertain'))
      throw new Error('Connection lost after sending');
    return Response.json({ data: state.runs[0].state });
  }
  const url = new URL(path, location.origin);
  if (url.pathname.includes('/blocks/')) {
    const id = decodeURIComponent(url.pathname.split('/').at(-1)!);
    const item = [...state.blocks, ...state.pending_interactions].find(
      (candidate) => candidate.block_id === id
    )!;
    const field = url.searchParams.get('field') ?? 'text';
    const full =
      bodies.get(`${id}:${field}`) ??
      String(item.fields[field as keyof typeof item.fields] ?? '');
    const bytes = new TextEncoder().encode(full);
    const offset = Math.min(
      Number(url.searchParams.get('offset') ?? 0),
      bytes.length
    );
    const end = Math.min(offset + 32768, bytes.length);
    return Response.json({
      data: {
        block: item,
        field,
        content: new TextDecoder().decode(bytes.slice(offset, end)),
        offset,
        next_offset: end < bytes.length ? end : null,
        total_length: bytes.length,
      },
    });
  }
  if (url.pathname.endsWith('/changes'))
    return Response.json({
      data: {
        ...state,
        tombstones: [],
        reset_required: false,
        has_more: false,
      },
    });
  return Response.json({ data: snapshot(url.searchParams.get('before')) });
}
export function updateBlock(item: ConversationBlock, pending = false) {
  item = { ...item, revision: String(++revision) };
  state.cursor.revision = String(revision);
  const existing = state.blocks.findIndex(
    (candidate) => candidate.block_id === item.block_id
  );
  if (existing < 0) state.blocks.push(item);
  else state.blocks[existing] = item;
  if (pending) state.pending_interactions = [item];
  for (const socket of sockets)
    socket.emit({
      type: 'changes',
      data: {
        ...state,
        blocks: [item],
        tombstones: [],
        reset_required: false,
        has_more: false,
      },
    });
}
export function append(text: string) {
  const item = state.blocks.find(
    (candidate) => candidate.block_id === 'answer'
  )!;
  const base_revision = item.revision;
  const previous = bodies.get('answer:text') ?? item.fields.text ?? '';
  const full = previous + text;
  bodies.set('answer:text', full);
  item.fields.text = full.slice(0, 16384);
  item.bodies = [
    { field: 'text', byte_length: full.length, preview_offset: 0 },
  ];
  item.revision = String(++revision);
  state.cursor.revision = String(revision);
  for (const socket of sockets) {
    socket.emit({
      type: 'append',
      data: {
        session_id: 'session',
        epoch: 'epoch',
        appends: [
          {
            block_id: 'answer',
            base_revision,
            revision: item.revision,
            field: 'text',
            text,
          },
        ],
      },
    });
    socket.emit({
      type: 'changes',
      data: {
        ...state,
        blocks: [],
        tombstones: [],
        reset_required: false,
        has_more: false,
      },
    });
  }
}
