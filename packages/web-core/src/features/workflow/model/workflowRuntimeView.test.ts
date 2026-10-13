import { describe, expect, it } from 'vitest';
import type {
  WorkflowNodeExecutionResponse,
  WorkflowRunResponse,
  WorkflowRunRuntimeView,
} from 'shared/types';
import {
  getDefaultWorkflowRuntimeNodeId,
  getWorkflowNodeActionGate,
  getWorkflowNodeExecutionForWork,
  getWorkflowNodeRuntimeSummary,
  getWorkflowNodeExecutionTarget,
  getWorkflowNodeWork,
  getWorkflowRunActionGate,
  getWorkflowRuntimeAttentionItems,
  getWorkflowRuntimeView,
  isWorkflowNodeProcessing,
} from './workflowRuntimeView';

function node(
  id: string,
  nodeId: string,
  nodeType: string,
  status: WorkflowNodeExecutionResponse['status'],
  iteration = 0n
): WorkflowNodeExecutionResponse {
  return {
    id,
    run_id: 'run-1',
    execution_id: null,
    node_id: nodeId,
    node_type: nodeType,
    iteration,
    status,
    input_text: null,
    output_text: null,
    session_id: null,
    orchestration_node_execution_id: null,
    agent_run_id: null,
    projection_status: null,
    execution_process_id: null,
    arena_group_id: null,
    tokens_used: null,
    cost_estimate: null,
    started_at: null,
    finished_at: null,
    error_text: null,
    created_at: '2026-06-25T00:00:00Z',
    updated_at: '2026-06-25T00:00:00Z',
  };
}

const baseRun = {
  id: 'run-1',
  orchestration_run_id: null,
  workflow_id: 'workflow-1',
  attempt_id: null,
  task_id: 'issue-1',
  workspace_id: 'workspace-1',
  trigger_source: 'manual',
  input_text: 'Build feature',
  output_text: null,
  status: 'running',
  started_at: '2026-06-25T00:00:00Z',
  finished_at: null,
  error_text: null,
  created_at: '2026-06-25T00:00:00Z',
  updated_at: '2026-06-25T00:00:00Z',
  nodes: [],
  split_batches: [],
  handoffs: [],
  delivery: null,
  request_path: null,
} satisfies WorkflowRunResponse;

describe('workflow runtime view', () => {
  it('uses the backend projection when present', () => {
    const runtimeView = {
      run_id: 'run-1',
      status: 'running',
      active_node_count: 1,
      pending_node_count: 0,
      waiting_node_count: 0,
      failed_node_count: 0,
      completed_node_count: 0,
      reused_node_count: 0,
      skipped_node_count: 0,
      node_work: [
        {
          node_id: 'agent',
          node_type: 'agent',
          iteration: 0n,
          status: 'running',
          pending_work_count: 0,
          starting_child_count: 0,
          active_execution_id: 'exec-1',
          active_session_id: 'session-1',
          orchestration_node_execution_id: 'orch-node-1',
          active_agent_run_id: 'agent-run-1',
          projection_status: 'current',
          active_started_at: '2026-06-25T00:00:00Z',
          active_elapsed_ms: 1000,
          active_slow: false,
          active_slow_threshold_ms: 300000,
          runtime_health: 'ok',
          can_open_session: true,
          can_retry: false,
          can_approve: false,
          can_reject: false,
          can_select_arena_winner: false,
          can_select_condition_branch: false,
          can_cancel_node: false,
          reused_results: [],
        },
      ],
    } satisfies WorkflowRunRuntimeView;

    expect(
      getWorkflowRuntimeView({ ...baseRun, runtime_view: runtimeView })
    ).toMatchObject({ node_work: [{ runtime_authority: 'current' }] });
  });

  it('builds a fallback projection from node executions', () => {
    const running = {
      ...node('exec-1', 'agent', 'agent', 'running'),
      session_id: 'session-1',
      execution_process_id: 'process-1',
      started_at: '2026-06-25T00:00:00Z',
    };
    const waiting = node('exec-2', 'approval', 'human_gate', 'awaiting_human');
    const failed = node('exec-3', 'fix', 'agent', 'failed');

    const view = getWorkflowRuntimeView(
      {
        ...baseRun,
        nodes: [running, waiting, failed],
      },
      { nowMs: Date.parse('2026-06-25T00:04:00Z') }
    );

    expect(view).toMatchObject({
      run_id: 'run-1',
      active_node_count: 1,
      waiting_node_count: 1,
      failed_node_count: 1,
    });
    expect(getDefaultWorkflowRuntimeNodeId(view)).toBe('approval');
    expect(isWorkflowNodeProcessing(getWorkflowNodeWork(view, 'agent'))).toBe(
      true
    );
    expect(
      getWorkflowNodeActionGate(getWorkflowNodeWork(view, 'agent'))
    ).toEqual({
      canOpenSession: false,
      canRetry: false,
      canApprove: false,
      canReject: false,
      canSelectArenaWinner: false,
      canSelectConditionBranch: false,
      canCancelNode: false,
    });
  });

  it('marks running nodes without canonical identity as starting and unknown', () => {
    const starting = {
      ...node('exec-1', 'starting', 'agent', 'running'),
      started_at: '2026-06-25T00:09:30Z',
    };
    const missing = {
      ...node('exec-2', 'missing', 'agent', 'running'),
      started_at: '2026-06-25T00:00:00Z',
    };

    const view = getWorkflowRuntimeView(
      {
        ...baseRun,
        nodes: [starting, missing],
      },
      {
        nowMs: Date.parse('2026-06-25T00:10:00Z'),
        activeSlowThresholdMs: 300000,
      }
    );

    expect(
      getWorkflowNodeRuntimeSummary(getWorkflowNodeWork(view, 'starting')!)
    ).toMatchObject({
      status: 'starting',
      runtimeHealth: 'starting',
      activeSlow: false,
      startingChildCount: 1,
    });
    expect(
      getWorkflowNodeRuntimeSummary(getWorkflowNodeWork(view, 'missing')!)
    ).toMatchObject({
      status: 'starting',
      runtimeHealth: 'unknown',
      activeSlow: true,
      startingChildCount: 1,
    });
  });

  it('uses the latest node iteration for slot-level work', () => {
    const view = getWorkflowRuntimeView({
      ...baseRun,
      nodes: [
        {
          ...node('exec-1', 'fan-in', 'agent', 'succeeded', 0n),
          updated_at: '2026-06-25T00:00:01Z',
        },
        {
          ...node('exec-2', 'fan-in', 'agent', 'pending', 1n),
          updated_at: '2026-06-25T00:00:02Z',
        },
      ],
    });

    expect(getWorkflowNodeWork(view, 'fan-in')).toMatchObject({
      iteration: 1n,
      status: 'pending',
      pending_work_count: 1,
    });
  });

  it('resolves terminal structural execution from canonical slot identity', () => {
    const older = {
      ...node('exec-older', 'approval', 'human_gate', 'failed', 1n),
      updated_at: '2026-06-25T00:00:01Z',
    };
    const latest = {
      ...node('exec-latest', 'approval', 'human_gate', 'succeeded', 1n),
      output_text: 'Approved',
      updated_at: '2026-06-25T00:00:02Z',
    };
    const runtimeView = {
      run_id: 'run-1',
      status: 'succeeded',
      active_node_count: 0,
      pending_node_count: 0,
      waiting_node_count: 0,
      failed_node_count: 0,
      completed_node_count: 1,
      reused_node_count: 0,
      skipped_node_count: 0,
      node_work: [
        {
          node_id: 'approval',
          node_type: 'human_gate',
          iteration: 1n,
          status: 'succeeded',
          pending_work_count: 0,
          starting_child_count: 0,
          active_execution_id: null,
          active_session_id: null,
          orchestration_node_execution_id: null,
          active_agent_run_id: null,
          projection_status: null,
          active_started_at: null,
          active_elapsed_ms: null,
          active_slow: false,
          active_slow_threshold_ms: 300000,
          runtime_health: 'ok',
          can_open_session: false,
          can_retry: false,
          can_approve: false,
          can_reject: false,
          can_select_arena_winner: false,
          can_select_condition_branch: false,
          can_cancel_node: false,
          reused_results: [],
        },
      ],
    } satisfies WorkflowRunRuntimeView;
    const run = {
      ...baseRun,
      status: 'succeeded',
      nodes: [older, latest],
      runtime_view: runtimeView,
    } satisfies WorkflowRunResponse;
    const view = getWorkflowRuntimeView(run);

    expect(
      getWorkflowNodeExecutionForWork(
        run,
        getWorkflowNodeWork(view, 'approval')
      )
    ).toEqual(latest);
  });

  it('projects attention only from current canonical work', () => {
    const runtimeView = {
      run_id: 'run-1',
      status: 'awaiting_human',
      active_node_count: 0,
      pending_node_count: 0,
      waiting_node_count: 1,
      failed_node_count: 1,
      completed_node_count: 0,
      reused_node_count: 0,
      skipped_node_count: 0,
      node_work: [
        {
          node_id: 'failed-agent',
          node_type: 'agent',
          iteration: 0n,
          status: 'failed',
          pending_work_count: 0,
          starting_child_count: 0,
          active_execution_id: 'exec-failed',
          active_session_id: 'session-failed',
          orchestration_node_execution_id: 'orch-failed',
          active_agent_run_id: 'agent-run-failed',
          projection_status: 'current',
          active_started_at: null,
          active_elapsed_ms: null,
          active_slow: false,
          active_slow_threshold_ms: 300000,
          runtime_health: 'ok',
          can_open_session: true,
          can_retry: true,
          can_approve: false,
          can_reject: false,
          can_select_arena_winner: false,
          can_select_condition_branch: false,
          can_cancel_node: false,
          reused_results: [],
        },
        {
          node_id: 'approval',
          node_type: 'human_gate',
          iteration: 0n,
          status: 'awaiting_human',
          pending_work_count: 0,
          starting_child_count: 0,
          active_execution_id: 'exec-approval',
          active_session_id: null,
          orchestration_node_execution_id: 'orch-approval',
          active_agent_run_id: null,
          projection_status: 'current',
          active_started_at: null,
          active_elapsed_ms: null,
          active_slow: false,
          active_slow_threshold_ms: 300000,
          runtime_health: 'ok',
          can_open_session: false,
          can_retry: false,
          can_approve: true,
          can_reject: true,
          can_select_arena_winner: false,
          can_select_condition_branch: false,
          can_cancel_node: false,
          reused_results: [],
        },
      ],
    } satisfies WorkflowRunRuntimeView;

    const current = getWorkflowRuntimeView({
      ...baseRun,
      runtime_view: runtimeView,
    });
    expect(getWorkflowRuntimeAttentionItems(current)).toEqual([
      {
        nodeId: 'approval',
        nodeType: 'human_gate',
        status: 'awaiting_human',
        kind: 'waiting',
      },
      {
        nodeId: 'failed-agent',
        nodeType: 'agent',
        status: 'failed',
        kind: 'failed',
      },
    ]);

    const fallback = getWorkflowRuntimeView({
      ...baseRun,
      nodes: [
        node('exec-fallback', 'approval', 'human_gate', 'awaiting_human'),
      ],
    });
    expect(getWorkflowRuntimeAttentionItems(fallback)).toEqual([]);
  });

  it('builds deep links only for canonically bound Agent and Arena Tasks', () => {
    const currentAgentWork = {
      runtime_authority: 'current',
      can_open_session: true,
    } as ReturnType<typeof getWorkflowNodeWork>;
    const agent = {
      ...node('exec-agent', 'agent', 'agent', 'running'),
      execution_id: 'task-agent',
      session_id: 'session-agent',
    };
    expect(getWorkflowNodeExecutionTarget(agent, currentAgentWork)).toEqual({
      kind: 'agent-session',
      taskId: 'task-agent',
      sessionId: 'session-agent',
    });

    const arena = {
      ...node('exec-arena', 'arena', 'arena', 'awaiting_arena'),
      execution_id: 'task-arena',
      arena_group_id: 'arena-group',
    };
    expect(
      getWorkflowNodeExecutionTarget(arena, {
        ...currentAgentWork,
        can_open_session: false,
      })
    ).toEqual({
      kind: 'arena',
      taskId: 'task-arena',
      arenaGroupId: 'arena-group',
    });

    expect(
      getWorkflowNodeExecutionTarget(
        { ...agent, node_type: 'condition' },
        currentAgentWork
      )
    ).toBeNull();
    expect(
      getWorkflowNodeExecutionTarget(
        { ...agent, execution_id: null },
        currentAgentWork
      )
    ).toBeNull();
    expect(
      getWorkflowNodeExecutionTarget(agent, {
        ...currentAgentWork,
        runtime_authority: 'unknown',
      })
    ).toBeNull();
  });

  it('exposes cancellation only for canonical cancellable run statuses', () => {
    expect(getWorkflowRunActionGate(baseRun)).toEqual({
      canCancel: true,
      cancellationPending: false,
    });
    expect(
      getWorkflowRunActionGate({ ...baseRun, status: 'cancelling' })
    ).toEqual({ canCancel: false, cancellationPending: true });
    expect(
      getWorkflowRunActionGate({ ...baseRun, status: 'succeeded' })
    ).toEqual({ canCancel: false, cancellationPending: false });
  });
});
