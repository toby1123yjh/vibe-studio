import { expect, test } from "@playwright/test";
import type {
  WorkflowNodeExecutionResponse,
  WorkflowNodeWorkView,
  WorkflowRunResponse,
} from "../../../shared/types";
import {
  getWorkflowNodeActionGate,
  getWorkflowNodeExecutionForWork,
  getWorkflowNodeExecutionTarget,
  getWorkflowNodeWork,
  getWorkflowRuntimeAttentionItems,
  getWorkflowRuntimeView,
} from "../../../packages/web-core/src/features/workflow/model/workflowRuntimeView";
import { buildWorkflowRunDashboardSummary } from "../../../packages/web-core/src/features/workflow/model/workflowRunView";

const noActions = {
  canOpenSession: false,
  canRetry: false,
  canApprove: false,
  canReject: false,
  canSelectArenaWinner: false,
  canSelectConditionBranch: false,
  canCancelNode: false,
};

function sourceExecution(): WorkflowNodeExecutionResponse {
  return {
    id: "source-execution",
    run_id: "source-run",
    execution_id: "source-task",
    node_id: "upstream",
    node_type: "agent",
    iteration: 3n,
    status: "succeeded",
    input_text: null,
    output_text: "old output",
    session_id: "source-session",
    orchestration_node_execution_id: "source-orchestration-execution",
    agent_run_id: "source-agent-run",
    projection_status: "current",
    execution_process_id: null,
    arena_group_id: null,
    tokens_used: 900n,
    cost_estimate: null,
    started_at: null,
    finished_at: null,
    error_text: null,
    created_at: "2026-10-04T00:00:00Z",
    updated_at: "2026-10-04T00:00:00Z",
  };
}

function planWork(status: "reused" | "skipped"): WorkflowNodeWorkView {
  return {
    node_id: "upstream",
    node_type: "agent",
    iteration: 3n,
    status,
    pending_work_count: 0,
    starting_child_count: 0,
    // Deliberately stale hints: these must not authorize a historical action.
    active_execution_id: "source-execution",
    active_session_id: "source-session",
    orchestration_node_execution_id: "source-orchestration-execution",
    active_agent_run_id: "source-agent-run",
    projection_status: "current",
    active_started_at: null,
    active_elapsed_ms: null,
    active_slow: false,
    active_slow_threshold_ms: 300000,
    runtime_health: "ok",
    can_open_session: true,
    can_retry: true,
    can_approve: true,
    can_reject: true,
    can_select_arena_winner: true,
    can_select_condition_branch: true,
    can_cancel_node: true,
    reused_results:
      status === "reused"
        ? [1, 3].map((iteration) => ({
            node_id: "upstream",
            iteration,
            source_run_id: "source-run",
            source_node_execution_id: `source-iteration-${iteration}`,
            output_text: `result ${iteration}`,
          }))
        : [],
  };
}

function currentRun(work: WorkflowNodeWorkView): WorkflowRunResponse {
  return {
    id: "current-run",
    orchestration_run_id: null,
    workflow_id: "workflow",
    attempt_id: "instance",
    task_id: "task",
    workspace_id: "workspace",
    trigger_source: "manual",
    input_text: "rework",
    output_text: null,
    status: "running",
    started_at: null,
    finished_at: null,
    error_text: null,
    created_at: "2026-10-04T00:00:00Z",
    updated_at: "2026-10-04T00:00:00Z",
    // Even accidental inclusion of the source cannot make reuse current work.
    nodes: [sourceExecution()],
    split_batches: [],
    handoffs: [],
    delivery: null,
    request_path: null,
    runtime_view: {
      run_id: "current-run",
      status: "running",
      active_node_count: 0,
      pending_node_count: 0,
      waiting_node_count: 0,
      failed_node_count: 0,
      completed_node_count: 0,
      reused_node_count: work.status === "reused" ? 1 : 0,
      skipped_node_count: work.status === "skipped" ? 1 : 0,
      node_work: [work],
    },
  };
}

for (const status of ["reused", "skipped"] as const) {
  test(`${status} plan facts cannot inherit actions or Task links from source hints`, () => {
    const run = currentRun(planWork(status));
    const view = getWorkflowRuntimeView(run);
    const work = getWorkflowNodeWork(view, "upstream");
    expect(work?.runtime_authority).toBe("current");
    expect(getWorkflowNodeActionGate(work)).toEqual(noActions);
    expect(getWorkflowNodeExecutionTarget(sourceExecution(), work)).toBeNull();
    expect(
      getWorkflowNodeExecutionTarget(
        {
          ...sourceExecution(),
          node_type: "arena",
          arena_group_id: "old-arena",
        },
        work,
      ),
    ).toBeNull();
    expect(getWorkflowRuntimeAttentionItems(view)).toEqual([]);
    if (status === "reused") {
      expect(getWorkflowNodeExecutionForWork(run, work)).toBeNull();
      expect(work?.reused_results).toEqual(planWork(status).reused_results);
    }
  });
}

for (const projectionStatus of ["projection_degraded", "rebuilding"] as const) {
  test(`reused lineage remains read-only when projection is ${projectionStatus}`, () => {
    const run = currentRun({
      ...planWork("reused"),
      projection_status: projectionStatus,
    });
    const view = getWorkflowRuntimeView(run);
    const work = getWorkflowNodeWork(view, "upstream");
    expect(view.authority).toBe("degraded");
    expect(work?.runtime_authority).toBe("degraded");
    expect(getWorkflowNodeExecutionForWork(run, work)).toBeNull();
    expect(getWorkflowNodeActionGate(work)).toEqual(noActions);
    expect(getWorkflowNodeExecutionTarget(sourceExecution(), work)).toBeNull();
    expect(work?.reused_results).toHaveLength(2);
  });
}

for (const pendingFreshNode of [false, true]) {
  test(`mixed lineage progress counts only current work${pendingFreshNode ? " while pending" : " at completion"}`, () => {
    const reused = planWork("reused");
    const skipped = { ...planWork("skipped"), node_id: "unchosen-route" };
    const fresh: WorkflowNodeWorkView = {
      ...planWork("skipped"),
      node_id: "fresh",
      status: "succeeded",
      iteration: 0n,
      reused_results: [],
      active_execution_id: "fresh-execution",
      active_session_id: "fresh-session",
      orchestration_node_execution_id: "fresh-orchestration-execution",
      active_agent_run_id: "fresh-agent-run",
    };
    const run = currentRun(reused);
    run.status = pendingFreshNode ? "running" : "succeeded";
    run.nodes = [
      {
        ...sourceExecution(),
        id: "fresh-execution",
        run_id: run.id,
        node_id: "fresh",
        iteration: 0n,
        tokens_used: 12n,
        cost_estimate: 0.25,
      },
    ];
    const work = [
      reused,
      { ...reused, node_id: "another-upstream" },
      skipped,
      fresh,
      ...(pendingFreshNode
        ? [{ ...fresh, node_id: "pending-fresh", status: "pending" as const }]
        : []),
    ];
    run.runtime_view = {
      run_id: run.id,
      status: run.status,
      active_node_count: 0,
      pending_node_count: pendingFreshNode ? 1 : 0,
      waiting_node_count: 0,
      failed_node_count: 0,
      completed_node_count: 1,
      reused_node_count: 2,
      skipped_node_count: 1,
      node_work: work,
    };
    const summary = buildWorkflowRunDashboardSummary(run);
    expect(summary).toMatchObject({
      totalSteps: pendingFreshNode ? 5 : 4,
      freshSteps: pendingFreshNode ? 2 : 1,
      completedSteps: 1,
      reusedSteps: 2,
      skippedSteps: 1,
      progressPercent: pendingFreshNode ? 50 : 100,
      totalTokens: 12,
      totalCostEstimate: 0.25,
    });
  });
}
