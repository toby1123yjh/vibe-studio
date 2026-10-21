import { expect, test } from '@playwright/test';
import { buildSync } from 'esbuild';
import { fileURLToPath } from 'node:url';
import type {
  AgentRunSummary,
  ConversationBlock,
  ConversationChangesPage,
  ConversationPage,
} from 'shared/types';
import type { AgentConversationTransport } from './agentConversationStore';

// Match the existing canonical model fixtures: bundle the real source in
// memory so generated enums resolve consistently in Node's mixed ESM tree.
const bundled = buildSync({
  entryPoints: [
    fileURLToPath(new URL('./agentConversationStore.ts', import.meta.url)),
  ],
  tsconfig: fileURLToPath(
    new URL('../../../../tsconfig.json', import.meta.url)
  ),
  bundle: true,
  platform: 'node',
  format: 'cjs',
  write: false,
});
const model = { exports: {} };
new Function('module', 'exports', bundled.outputFiles[0].text)(
  model,
  model.exports
);
const {
  AgentConversationStore,
  getAgentConversationStore,
  retainAgentConversationStore,
} = model.exports as typeof import('./agentConversationStore');

const run = (status = 'running'): AgentRunSummary =>
  ({
    agent_run_id: 'run-a',
    session_id: 'session',
    turn_id: 'turn-a',
    created_at: '2026-10-20T00:00:00Z',
    updated_at: '2026-10-20T00:00:00Z',
    state: {
      session_id: 'session',
      agent_run_id: 'run-a',
      turn_id: 'turn-a',
      status,
      projection_status: 'current',
      last_run_attempt_id: 'attempt-a',
      last_run_attempt_number: 1,
      updated_at: '2026-10-20T00:00:00Z',
    },
  }) as AgentRunSummary;
const block = (
  id: string,
  revision = '1',
  text = 'hello'
): ConversationBlock => ({
  block_id: id,
  agent_run_id: 'run-a',
  run_attempt_id: 'attempt-a',
  kind: 'text',
  role: 'assistant',
  native_id: null,
  parent_block_id: null,
  position: id.padStart(5, '0'),
  revision,
  fields: { text },
  bodies: [
    {
      field: 'text',
      byte_length: new TextEncoder().encode(text).byteLength,
      preview_offset: 0,
    },
  ],
  finished: false,
  finish_reason: null,
  source_event_id: `event-${id}`,
  created_at: '2026-10-20T00:00:00Z',
  updated_at: '2026-10-20T00:00:00Z',
});
const page = (
  blocks: ConversationBlock[],
  revision = '1',
  epoch = 'epoch-a'
): ConversationPage => ({
  session_id: 'session',
  cursor: { epoch, revision },
  blocks,
  runs: [run()],
  pending_interactions: [],
  previous_cursor: null,
  has_more: false,
});
const changes = (
  blocks: ConversationBlock[],
  revision: string
): ConversationChangesPage => ({
  ...page(blocks, revision),
  tombstones: [],
  reset_required: false,
});

test('token updates notify only the block and preserve structural/history/run references', () => {
  const store = new AgentConversationStore('session', null);
  store.applyPage(page([block('1')]), true);
  const initial = store.getSnapshot();
  let metadata = 0;
  let tokens = 0;
  store.subscribe(() => ++metadata);
  store.subscribeBlock('1', () => ++tokens);
  store.applyChanges(changes([block('1', '2', 'hello world')], '2'));
  expect(store.getSnapshot()).toBe(initial);
  expect(store.getCursor()?.revision).toBe('2');
  expect(metadata).toBe(0);
  expect(tokens).toBe(1);
  const settled = changes([], '3');
  settled.runs = [run('succeeded')];
  const oldBlock = store.getBlock('1');
  store.applyChanges(settled);
  expect(store.getSnapshot().conversation.isRunning).toBe(false);
  expect(store.getBlock('1')).not.toBe(oldBlock);
});

test('backward pagination neither regresses new content nor advances the live cursor', () => {
  const store = new AgentConversationStore('session', null);
  store.applyPage(page([block('2', '5')], '5'), true);
  store.applyChanges(changes([block('2', '8', 'new')], '8'));
  const historic = page([block('1', '2'), block('2', '4', 'old')], '12');
  store.applyPage(historic);
  expect(store.getCursor()?.revision).toBe('8');
  expect(store.getBlock('2')?.fields.text).toBe('new');
  expect(
    store
      .getSnapshot()
      .conversation.entries.map((entry) => entry.conversationBlockId)
  ).toEqual(['1', '2']);
});

test('epoch reset discards old baselines, tombstones remove rows, and stale changes cannot rewind', () => {
  const store = new AgentConversationStore('session', null);
  store.applyPage(page([block('1')]), true);
  store.applyPage(page([block('2')], '2', 'epoch-b'), true);
  expect(store.getBlock('1')).toBeUndefined();
  expect(store.applyChanges(changes([block('1', '5')], '5'))).toBe(false);
  const removal = {
    ...changes([], '3'),
    cursor: { epoch: 'epoch-b', revision: '3' },
    tombstones: ['2'],
  };
  store.applyChanges(removal);
  expect(store.getSnapshot().conversation.entries).toHaveLength(0);
  store.applyChanges({
    ...removal,
    cursor: { epoch: 'epoch-b', revision: '2' },
    blocks: [block('2', '2')],
    tombstones: [],
  });
  expect(store.getBlock('2')).toBeUndefined();
});

test('newer backward-page metadata cannot regress during live catch-up', () => {
  const store = new AgentConversationStore('session', null);
  store.applyPage(page([block('2', '8')], '8'), true);
  const historic = page([block('1', '2')], '12');
  historic.runs = [run('succeeded')];
  store.applyPage(historic);
  expect(store.getCursor()?.revision).toBe('8');
  expect(store.getSnapshot().conversation.isRunning).toBe(false);
  store.applyChanges(changes([block('2', '10', 'catching up')], '10'));
  expect(store.getCursor()?.revision).toBe('10');
  expect(store.getBlock('2')?.fields.text).toBe('catching up');
  expect(store.getSnapshot().conversation.isRunning).toBe(false);
  // A new projection epoch starts its own version space.
  store.applyPage(page([block('3')], '1', 'epoch-b'), true);
  expect(store.getSnapshot().conversation.isRunning).toBe(true);
});

test('an unknown historical attempt number stays unknown', () => {
  const store = new AgentConversationStore('session', null);
  store.applyPage(
    page([{ ...block('1'), run_attempt_id: 'earlier-attempt' }]),
    true
  );
  expect(
    store.getSnapshot().conversation.entries[0].canonical?.runAttemptNumber
  ).toBeNull();
});

test('append uses revision chains and keeps a bounded head without putting suffixes at a false offset', () => {
  const store = new AgentConversationStore('session', null);
  const initial = block('1');
  initial.bodies[0].byte_length = 100000;
  store.applyPage(page([initial]), true);
  const before = store.getSnapshot();
  store.applyAppend({
    session_id: 'session',
    epoch: 'epoch-a',
    appends: [
      {
        block_id: '1',
        base_revision: '1',
        revision: '2',
        field: 'text',
        text: '中文',
      },
    ],
  });
  expect(store.getBlock('1')?.fields.text).toBe('hello');
  expect(Number(store.getBlock('1')?.bodies[0].byte_length)).toBe(100006);
  expect(store.getBlock('1')?.revision).toBe('2');
  expect(store.getSnapshot()).toBe(before);
  expect(store.getCursor()?.revision).toBe('1');
});

test('append mismatch reloads a full baseline and a same-revision final baseline is accepted', async () => {
  let reads = 0;
  const recovered = block('1', '4', 'recovered');
  const transport = {
    detail: async () => {
      ++reads;
      return { block: recovered };
    },
  } as unknown as AgentConversationTransport;
  const store = new AgentConversationStore('session', null, transport);
  store.applyPage(page([block('1')]), true);
  store.applyAppend({
    session_id: 'session',
    epoch: 'epoch-a',
    appends: [
      {
        block_id: '1',
        base_revision: '3',
        revision: '4',
        field: 'text',
        text: 'suffix',
      },
    ],
  });
  await expect.poll(() => store.getBlock('1')?.fields.text).toBe('recovered');
  expect(reads).toBe(1);
  store.applyChanges(
    changes([{ ...recovered, finished: true, finish_reason: 'stop' }], '4')
  );
  expect(store.getBlock('1')?.finished).toBe(true);
});

test('loaded window is bounded and reading older pages does not pull in the live tail', () => {
  const store = new AgentConversationStore('session', null);
  store.applyPage(page([block('1')]), true);
  store.applyChanges(
    changes(
      Array.from({ length: 600 }, (_, index) => block(String(index + 1), '2')),
      '2'
    )
  );
  expect(store.getSnapshot().conversation.entries).toHaveLength(500);
  store.applyPage({
    ...page(
      Array.from({ length: 50 }, (_, index) => block(String(index + 1), '2')),
      '2'
    ),
    has_more: true,
    previous_cursor: '00001',
  });
  expect(store.getSnapshot().hasNewer).toBe(true);
  store.applyChanges(changes([block('601', '3')], '3'));
  expect(store.getSnapshot().conversation.entries).toHaveLength(500);
  expect(store.getBlock('601')).toBeUndefined();
  expect(store.getBlock('600')).toBeUndefined();
});

test('host/session registry shares exactly one stream for multiple retained readers', async () => {
  let streams = 0;
  const transport = {
    page: async () => page([block('1')]),
    openStream: async () => {
      ++streams;
      return { close() {} };
    },
  } as unknown as AgentConversationTransport;
  const store = getAgentConversationStore('session', 'host-a', transport);
  expect(getAgentConversationStore('session', 'host-a', transport)).toBe(store);
  expect(getAgentConversationStore('session', 'host-b', transport)).not.toBe(
    store
  );
  const releaseA = retainAgentConversationStore(store);
  const releaseB = retainAgentConversationStore(store);
  await expect.poll(() => streams).toBe(1);
  releaseA();
  releaseB();
  await Promise.resolve();
  expect(store.getSnapshot().isConnected).toBe(false);
});
