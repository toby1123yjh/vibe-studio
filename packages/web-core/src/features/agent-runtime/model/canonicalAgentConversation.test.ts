import { expect, test as it } from '@playwright/test';
import { buildSync } from 'esbuild';
import { fileURLToPath } from 'node:url';
import type {
  AgentEventEnvelope,
  AgentEventPayload,
  AgentRunSummary,
  RunState,
} from 'shared/types';

// Bundle the real pure models in memory so Node's mixed CommonJS/ESM handling
// of generated shared/types.ts cannot replace or hide the production enums.
const modelModule = { exports: {} };
const bundle = buildSync({
  stdin: {
    contents: `
      export { projectCanonicalAgentConversation } from './canonicalAgentConversation';
      export { buildCanonicalAgentSessionTimeline } from './canonicalAgentSessionTimeline';
      export { emptyCanonicalAgentTimeline, mergeCanonicalAgentTimeline } from './canonicalAgentTimeline';
      export { AgentRuntimeToolStatus } from 'shared/types';
    `,
    resolveDir: fileURLToPath(new URL('.', import.meta.url)),
    loader: 'ts',
  },
  tsconfig: fileURLToPath(
    new URL('../../../../tsconfig.json', import.meta.url)
  ),
  bundle: true,
  platform: 'node',
  format: 'cjs',
  write: false,
});
new Function('module', 'exports', bundle.outputFiles[0].text)(
  modelModule,
  modelModule.exports
);
const {
  projectCanonicalAgentConversation,
  buildCanonicalAgentSessionTimeline,
  emptyCanonicalAgentTimeline,
  mergeCanonicalAgentTimeline,
  AgentRuntimeToolStatus,
} = modelModule.exports as typeof import('./canonicalAgentConversation') &
  typeof import('./canonicalAgentSessionTimeline') &
  typeof import('./canonicalAgentTimeline') &
  Pick<typeof import('shared/types'), 'AgentRuntimeToolStatus'>;

const SESSION_ID = 'session-1';
const describe = it.describe;

function state(runId: string, status: RunState['status']): RunState {
  return {
    state_schema_version: 1,
    reducer_version: 1,
    session_id: SESSION_ID,
    agent_run_id: runId,
    turn_id: `turn-${runId}`,
    status,
    projection_status: 'current',
    last_run_attempt_id: `attempt-${runId}`,
    last_run_attempt_number: 1,
    last_event_sequence: 0n,
    last_event_id: null,
    provider_session: null,
    terminal_output: null,
    last_error: null,
    unknown_event_count: 0n,
    updated_at: '2026-08-14T00:00:00Z',
  };
}

function event(
  runId: string,
  sequence: number,
  payload: AgentEventPayload
): AgentEventEnvelope {
  return {
    schema_version: 1,
    payload_version: 1,
    event_id: `${runId}-event-${sequence}`,
    session_id: SESSION_ID,
    agent_run_id: runId,
    turn_id: `turn-${runId}`,
    run_attempt_id: `attempt-${runId}`,
    run_attempt_number: 1,
    sequence: BigInt(sequence),
    correlation_id: `correlation-${runId}`,
    timestamp: `2026-08-14T00:00:${String(sequence).padStart(2, '0')}Z`,
    native_refs: [],
    payload,
  };
}

function projectRuns(
  runs: Array<{
    runId: string;
    status: RunState['status'];
    events: AgentEventEnvelope[];
  }>
) {
  const summaries: AgentRunSummary[] = [];
  const timelines = new Map();

  runs.forEach((run, index) => {
    const runState = state(run.runId, run.status);
    summaries.push({
      agent_run_id: run.runId,
      session_id: SESSION_ID,
      turn_id: `turn-${run.runId}`,
      state: runState,
      created_at: `2026-08-14T00:0${index}:00Z`,
      updated_at: `2026-08-14T00:0${index}:00Z`,
    });
    timelines.set(
      run.runId,
      mergeCanonicalAgentTimeline(
        emptyCanonicalAgentTimeline(),
        run.events,
        runState
      )
    );
  });

  return projectCanonicalAgentConversation(
    buildCanonicalAgentSessionTimeline(SESSION_ID, summaries, timelines)
  );
}

describe('projectCanonicalAgentConversation', () => {
  it('projects canonical messages without inventing execution-process identity', () => {
    const runId = 'run-message';
    const projection = projectRuns([
      {
        runId,
        status: 'succeeded',
        events: [
          event(runId, 1, {
            type: 'message',
            data: {
              message: {
                message_id: 'user-1',
                role: 'user',
                content: 'Build it',
              },
              final_output: false,
            },
          }),
          event(runId, 2, {
            type: 'message',
            data: {
              message: {
                message_id: 'assistant-1',
                role: 'assistant',
                content: 'Hel',
              },
              final_output: false,
            },
          }),
          event(runId, 3, {
            type: 'message',
            data: {
              message: {
                message_id: 'assistant-1',
                role: 'assistant',
                content: 'Hello',
              },
              final_output: true,
            },
          }),
        ],
      },
    ]);

    expect(
      projection.entries.map((entry) => ({
        type:
          entry.type === 'NORMALIZED_ENTRY'
            ? entry.content.entry_type.type
            : entry.type,
        content: entry.type === 'NORMALIZED_ENTRY' ? entry.content.content : '',
      }))
    ).toEqual([
      { type: 'user_message', content: 'Build it' },
      { type: 'assistant_message', content: 'Hello' },
    ]);
    expect(
      projection.entries.every(
        (entry) => entry.executionProcessId === undefined
      )
    ).toBe(true);
  });

  it('keeps one assistant reply when native metadata separates deltas from completion', () => {
    const runId = 'run-stream-metadata';
    const projection = projectRuns([
      {
        runId,
        status: 'succeeded',
        events: [
          event(runId, 1, {
            type: 'message',
            data: {
              message: {
                message_id: 'reply',
                role: 'assistant',
                content: 'SMOKE_',
              },
              final_output: false,
            },
          }),
          event(runId, 2, {
            type: 'provider_extension',
            data: {
              provider_namespace: 'codex',
              provider_event: 'turn/moderationMetadata',
              payload: null,
            },
          }),
          event(runId, 3, {
            type: 'file_changes',
            data: {
              tool_call_id: 'patch-1',
              changes: [{ path: 'smoke.txt', change_type: 'modified' }],
            },
          }),
          event(runId, 4, {
            type: 'message',
            data: {
              message: {
                message_id: 'reply',
                role: 'assistant',
                content: 'OK',
              },
              final_output: false,
            },
          }),
          event(runId, 5, {
            type: 'lifecycle_changed',
            data: { status: 'running' },
          }),
          event(runId, 6, {
            type: 'message',
            data: {
              message: {
                message_id: 'reply',
                role: 'assistant',
                content: 'SMOKE_OK',
              },
              final_output: true,
            },
          }),
        ],
      },
    ]);
    expect(projection.entries).toHaveLength(1);
    const [reply] = projection.entries;
    expect(reply.type === 'NORMALIZED_ENTRY' && reply.content.content).toBe(
      'SMOKE_OK'
    );
    expect(reply.canonical?.eventIds).toEqual([
      `${runId}-event-1`,
      `${runId}-event-4`,
      `${runId}-event-6`,
    ]);
  });

  it('does not merge the same message id across process attempts', () => {
    const runId = 'run-retried-message';
    const projection = projectRuns([
      {
        runId,
        status: 'succeeded',
        events: [
          event(runId, 1, {
            type: 'message',
            data: {
              message: {
                message_id: 'reply',
                role: 'assistant',
                content: 'First attempt',
              },
              final_output: false,
            },
          }),
          {
            ...event(runId, 2, {
              type: 'message',
              data: {
                message: {
                  message_id: 'reply',
                  role: 'assistant',
                  content: 'Second attempt',
                },
                final_output: true,
              },
            }),
            run_attempt_id: 'attempt-2',
            run_attempt_number: 2,
            sequence: 1n,
          },
        ],
      },
    ]);
    expect(projection.entries).toHaveLength(2);
    expect(
      projection.entries.map(
        (entry) => entry.type === 'NORMALIZED_ENTRY' && entry.content.content
      )
    ).toEqual(['First attempt', 'Second attempt']);
  });

  it('uses the final message snapshot when it corrects streamed content', () => {
    const runId = 'run-final-snapshot';
    const projection = projectRuns([
      {
        runId,
        status: 'succeeded',
        events: [
          event(runId, 1, {
            type: 'message',
            data: {
              message: {
                message_id: 'reply',
                role: 'assistant',
                content: 'draft',
              },
              final_output: false,
            },
          }),
          event(runId, 2, {
            type: 'message',
            data: {
              message: {
                message_id: 'reply',
                role: 'assistant',
                content: 'final',
              },
              final_output: true,
            },
          }),
        ],
      },
    ]);
    expect(projection.entries).toHaveLength(1);
    const [reply] = projection.entries;
    expect(reply.type === 'NORMALIZED_ENTRY' && reply.content.content).toBe(
      'final'
    );
  });

  it('updates one visible status row for repeated provider retry notices', () => {
    const runId = 'run-retry';
    const projection = projectRuns([
      {
        runId,
        status: 'running',
        events: [
          event(runId, 1, {
            type: 'message',
            data: {
              message: {
                message_id: 'retry-status',
                role: 'system',
                content: 'Retrying (attempt 1/10)',
              },
              final_output: false,
            },
          }),
          event(runId, 2, {
            type: 'provider_extension',
            data: {
              provider_namespace: 'claude_code',
              provider_event: 'retry_metadata',
              payload: null,
            },
          }),
          event(runId, 3, {
            type: 'message',
            data: {
              message: {
                message_id: 'retry-status',
                role: 'system',
                content: 'Retrying (attempt 2/10)',
              },
              final_output: false,
            },
          }),
        ],
      },
    ]);
    const statusEntries = projection.entries.filter(
      (entry) =>
        entry.type === 'NORMALIZED_ENTRY' &&
        entry.content.entry_type.type === 'system_message'
    );

    expect(statusEntries).toHaveLength(1);
    expect(statusEntries[0]?.type).toBe('NORMALIZED_ENTRY');
    if (statusEntries[0]?.type === 'NORMALIZED_ENTRY') {
      expect(statusEntries[0].content.content).toBe('Retrying (attempt 2/10)');
      expect(statusEntries[0].canonical?.eventIds).toEqual([
        `${runId}-event-1`,
        `${runId}-event-3`,
      ]);
    }
  });

  it('aggregates tools by run and tool-call id and closes them at terminal state', () => {
    const firstRun = 'run-tool-1';
    const secondRun = 'run-tool-2';
    const projection = projectRuns([
      {
        runId: firstRun,
        status: 'succeeded',
        events: [
          event(firstRun, 1, {
            type: 'tool_call',
            data: {
              tool_call_id: 'shared-tool-id',
              tool_name: 'Shell',
              status: AgentRuntimeToolStatus.running,
              arguments: { command: 'pwd' },
              result: null,
            },
          }),
          event(firstRun, 2, {
            type: 'tool_call',
            data: {
              tool_call_id: 'shared-tool-id',
              tool_name: 'Shell',
              status: AgentRuntimeToolStatus.succeeded,
              arguments: { command: 'pwd' },
              result: '/workspace',
            },
          }),
        ],
      },
      {
        runId: secondRun,
        status: 'succeeded',
        events: [
          event(secondRun, 1, {
            type: 'tool_call',
            data: {
              tool_call_id: 'shared-tool-id',
              tool_name: 'Shell',
              status: AgentRuntimeToolStatus.running,
              arguments: { command: 'ls' },
              result: null,
            },
          }),
        ],
      },
    ]);
    const tools = projection.entries.filter(
      (entry) =>
        entry.type === 'NORMALIZED_ENTRY' &&
        entry.content.entry_type.type === 'tool_use'
    );

    expect(tools).toHaveLength(2);
    expect(tools.map((entry) => entry.canonical?.agentRunId)).toEqual([
      firstRun,
      secondRun,
    ]);
    expect(tools[0]?.canonical?.eventIds).toHaveLength(2);
    expect(tools.every((entry) => entry.canonical?.active === false)).toBe(
      true
    );
  });

  it('resolves canonical approval and input controls in place', () => {
    const runId = 'run-control';
    const projection = projectRuns([
      {
        runId,
        status: 'succeeded',
        events: [
          event(runId, 1, {
            type: 'approval_requested',
            data: {
              approval_id: 'approval-1',
              tool_call_id: null,
              tool_name: 'Write',
            },
          }),
          event(runId, 2, {
            type: 'approval_resolved',
            data: {
              approval_id: 'approval-1',
              approved: false,
              reason: 'Not now',
            },
          }),
          event(runId, 3, {
            type: 'input_requested',
            data: { input_id: 'input-1', prompt: 'Which branch?' },
          }),
          event(runId, 4, {
            type: 'input_resolved',
            data: { input_id: 'input-1', answered: true },
          }),
        ],
      },
    ]);
    const controls = projection.entries.filter(
      (entry) =>
        entry.type === 'NORMALIZED_ENTRY' &&
        entry.content.entry_type.type === 'tool_use'
    );

    expect(controls).toHaveLength(2);
    if (
      controls[0]?.type !== 'NORMALIZED_ENTRY' ||
      controls[0].content.entry_type.type !== 'tool_use' ||
      controls[1]?.type !== 'NORMALIZED_ENTRY' ||
      controls[1].content.entry_type.type !== 'tool_use'
    ) {
      throw new Error('Expected canonical control entries');
    }
    expect(controls[0].content.entry_type.status).toEqual({
      status: 'denied',
      reason: 'Not now',
    });
    expect(controls[0].canonical?.eventIds).toHaveLength(2);
    expect(controls[1].content.entry_type.status).toEqual({
      status: 'success',
    });
    expect(controls[1].canonical?.eventIds).toHaveLength(2);
  });

  it('reports token totals without inventing a context window', () => {
    const runId = 'run-usage';
    const projection = projectRuns([
      {
        runId,
        status: 'succeeded',
        events: [
          event(runId, 1, {
            type: 'token_usage',
            data: {
              input_tokens: 12n,
              output_tokens: 5n,
              cached_input_tokens: 3n,
            },
          }),
        ],
      },
    ]);
    const usage = projection.entries.find(
      (entry) =>
        entry.type === 'NORMALIZED_ENTRY' &&
        entry.content.entry_type.type === 'token_usage_info'
    );

    if (
      usage?.type !== 'NORMALIZED_ENTRY' ||
      usage.content.entry_type.type !== 'token_usage_info'
    ) {
      throw new Error('Expected token usage entry');
    }
    expect(usage.content.entry_type.total_tokens).toBe(17);
    expect(usage.content.entry_type.model_context_window).toBe(0);
  });
});
