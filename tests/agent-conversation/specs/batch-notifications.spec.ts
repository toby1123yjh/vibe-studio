import { expect, test } from '@playwright/test';
import { buildSync } from 'esbuild';
import path from 'node:path';
import type {
  ConversationBlock,
  ConversationChangesPage,
  ConversationPage,
} from '../../../shared/types';

type StoreModule =
  typeof import('../../../packages/web-core/src/features/agent-runtime/model/agentConversationStore');
type Fixture = Pick<
  typeof import('../fixture/transport'),
  'block' | 'state' | 'requests'
>;

const storePath = path.resolve(
  'packages/web-core/src/features/agent-runtime/model/agentConversationStore.ts'
);
const storeUrl = `/@fs/${storePath.replaceAll('\\', '/')}`;
const apiUrl = `/@fs/${path
  .resolve('packages/web-core/src/shared/lib/agentConversationApi.ts')
  .replaceAll('\\', '/')}`;
const bundled = buildSync({
  entryPoints: [storePath],
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
const { AgentConversationStore } = model.exports as StoreModule;

function block(id: string, revision = '1'): ConversationBlock {
  return {
    block_id: id,
    agent_run_id: 'run',
    run_attempt_id: 'attempt',
    kind: 'text',
    role: 'assistant',
    native_id: null,
    parent_block_id: null,
    position: id,
    revision,
    fields: { text: id },
    bodies: [],
    finished: false,
    finish_reason: null,
    source_event_id: id,
    created_at: '2026-10-26T00:00:00Z',
    updated_at: '2026-10-26T00:00:00Z',
  };
}

function page(blocks: ConversationBlock[], revision = '1'): ConversationPage {
  return {
    session_id: 'session',
    cursor: { epoch: 'epoch', revision },
    blocks,
    runs: [],
    pending_interactions: [],
    previous_cursor: null,
    has_more: false,
  };
}

test('a collapsed 500-tool group reads each tool once per output changes page', async ({
  page,
}) => {
  await page.goto('/?list=1');
  await expect(page.locator('.list-host')).toContainText('Message 600');
  await page.evaluate(
    async ({ storeUrl, apiUrl }) => {
      const fixture = (window as unknown as { conversationFixture: Fixture })
        .conversationFixture;
      const { getAgentConversationStore } = (await import(
        storeUrl
      )) as StoreModule;
      const { agentConversationApi } = (await import(
        apiUrl
      )) as typeof import('../../../packages/web-core/src/shared/lib/agentConversationApi');
      const store = getAgentConversationStore(
        'session',
        'host-a',
        agentConversationApi
      );
      const blocks = Array.from({ length: 500 }, (_, index) => ({
        ...fixture.block(String(index).padStart(5, '0'), 'tool', {
          tool_name: 'Audit command',
          status: 'succeeded',
          output_text: 'initial output',
        }),
        revision: '1000',
        finished: true,
      }));
      store.applyPage(
        {
          ...fixture.state,
          blocks,
          cursor: { epoch: 'epoch', revision: '1000' },
        },
        true
      );
    },
    { storeUrl, apiUrl }
  );
  const group = page.getByRole('button', { name: /Tool activity/ });
  await expect(group).toContainText('Audit command');
  await expect(group).toHaveAttribute('aria-expanded', 'false');

  const counts = await page.evaluate(
    async ({ storeUrl, apiUrl }) => {
      const fixture = (window as unknown as { conversationFixture: Fixture })
        .conversationFixture;
      const { getAgentConversationStore } = (await import(
        storeUrl
      )) as StoreModule;
      const { agentConversationApi } = (await import(
        apiUrl
      )) as typeof import('../../../packages/web-core/src/shared/lib/agentConversationApi');
      const store = getAgentConversationStore(
        'session',
        'host-a',
        agentConversationApi
      );
      const getBlock = store.getBlock;
      const blocks = store.getSnapshot().conversation.entries.map((entry) => {
        const block = getBlock(entry.conversationBlockId!)!;
        return {
          ...block,
          revision: '1001',
          fields: { ...block.fields, output_text: 'updated output' },
        };
      });
      let reads = 0;
      store.getBlock = (id) => {
        ++reads;
        return getBlock(id);
      };
      try {
        store.applyChanges({
          ...fixture.state,
          blocks,
          cursor: { epoch: 'epoch', revision: '1001' },
          tombstones: [],
          reset_required: false,
        });
        return {
          reads,
          details: fixture.requests.filter((request) =>
            request.path.includes('/blocks/')
          ).length,
        };
      } finally {
        store.getBlock = getBlock;
      }
    },
    { storeUrl, apiUrl }
  );
  expect(counts.reads).toBe(500);
  expect(counts.details).toBe(0);
  await expect(page.locator('[data-conversation-block]')).toHaveCount(0);
});

test('snapshot and terminal changes notify shared subscribers after the complete page', () => {
  const store = new AgentConversationStore('session', null);
  store.applyPage(page([block('a'), block('b')]), true);
  const observations: Array<{
    revision?: string;
    first?: string;
    second?: string;
    pending: number;
    status?: string;
  }> = [];
  const observe = () =>
    observations.push({
      revision: store.getCursor()?.revision,
      first: store.getBlock('a')?.revision,
      second: store.getBlock('b')?.revision,
      pending: store.getSnapshot().pendingInteractions.length,
      status: store.getSnapshot().conversation.latestStatus ?? undefined,
    });
  store.subscribe(observe);
  store.subscribeBlock('a', observe);
  store.subscribeBlock('b', observe);
  store.applyPage(page([block('a', '2'), block('b', '2')], '2'), true);
  expect(observations).toEqual([
    { revision: '2', first: '2', second: '2', pending: 0 },
  ]);

  const waiting = page([block('request', '3')], '3');
  waiting.pending_interactions = waiting.blocks;
  store.applyChanges({ ...waiting, tombstones: [], reset_required: false });
  expect(observations.at(-1)?.pending).toBe(1);
  observations.length = 0;

  const terminal: ConversationChangesPage = {
    ...page(
      [block('a', '4'), block('b', '4')].map((block) => ({
        ...block,
        finished: true,
        fields: { text: 'Final output' },
      })),
      '4'
    ),
    runs: [
      {
        agent_run_id: 'run',
        session_id: 'session',
        turn_id: 'turn',
        created_at: '2026-10-26T00:00:00Z',
        updated_at: '2026-10-26T00:00:00Z',
        state: {
          agent_run_id: 'run',
          session_id: 'session',
          turn_id: 'turn',
          status: 'succeeded',
          projection_status: 'current',
          last_run_attempt_id: 'attempt',
          last_run_attempt_number: 1,
          updated_at: '2026-10-26T00:00:00Z',
        },
      } as ConversationPage['runs'][number],
    ],
    tombstones: ['request'],
    reset_required: false,
  };
  store.applyChanges(terminal);
  expect(observations).toEqual([
    {
      revision: '4',
      first: '4',
      second: '4',
      pending: 0,
      status: 'succeeded',
    },
  ]);
  expect(store.getBlock('a')?.fields.text).toBe('Final output');
  expect(store.getBlock('request')).toBeUndefined();
});

test('an append batch retains every delta without broadcasting session metadata', () => {
  const store = new AgentConversationStore('session', null);
  store.applyPage(page([block('a'), block('b')]), true);
  const snapshot = store.getSnapshot();
  let metadata = 0;
  const observed: string[] = [];
  const observe = () =>
    observed.push(
      `${store.getBlock('a')?.fields.text}:${store.getBlock('b')?.fields.text}`
    );
  store.subscribe(() => ++metadata);
  store.subscribeBlock('a', observe);
  store.subscribeBlock('b', observe);
  store.applyAppend({
    session_id: 'session',
    epoch: 'epoch',
    appends: [
      {
        block_id: 'a',
        base_revision: '1',
        revision: '2',
        field: 'text',
        text: '1',
      },
      {
        block_id: 'a',
        base_revision: '2',
        revision: '3',
        field: 'text',
        text: '2',
      },
      {
        block_id: 'b',
        base_revision: '1',
        revision: '4',
        field: 'text',
        text: '3',
      },
    ],
  });
  expect(observed).toEqual(['a12:b3']);
  expect(metadata).toBe(0);
  expect(store.getSnapshot()).toBe(snapshot);
  expect(store.getCursor()?.revision).toBe('1');
});
