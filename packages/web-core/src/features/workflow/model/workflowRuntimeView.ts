import type {
  NodeExecutionStatus,
  ProjectionStatus,
  WorkflowNodeExecutionResponse,
  WorkflowNodeWorkStatus,
  WorkflowNodeWorkView,
  WorkflowRunResponse,
  WorkflowRunRuntimeView,
  WorkflowSplitBatchView,
  WorkflowSplitSubtaskView,
  WorkflowSubmission,
} from 'shared/types';

export const WORKFLOW_NODE_ACTIVE_SLOW_THRESHOLD_MS = 5 * 60 * 1000;

export interface WorkflowRuntimeViewOptions {
  nowMs?: number;
  activeSlowThresholdMs?: number;
}

/**
 * Frontend-only metadata attached while crossing the workflow API boundary.
 * It is deliberately not serialized back to the server and prevents a
 * missing/degraded canonical projection from silently becoming actionable UI.
 */
export type WorkflowRuntimeAuthority = 'current' | 'degraded' | 'unknown';

export type CanonicalWorkflowNodeWorkView = WorkflowNodeWorkView & {
  orchestration_node_execution_id?: string | null;
  active_agent_run_id?: string | null;
  projection_status?: ProjectionStatus | null;
  runtime_authority?: WorkflowRuntimeAuthority;
};

type CanonicalWorkflowNodeExecution = WorkflowNodeExecutionResponse & {
  orchestration_node_execution_id?: string | null;
  agent_run_id?: string | null;
  projection_status?: ProjectionStatus | null;
};

export type WorkflowRuntimeProjection = Omit<
  WorkflowRunRuntimeView,
  'node_work'
> & {
  node_work: CanonicalWorkflowNodeWorkView[];
  authority: WorkflowRuntimeAuthority;
};

export interface WorkflowNodeActionGate {
  canOpenSession: boolean;
  canRetry: boolean;
  canApprove: boolean;
  canReject: boolean;
  canSelectArenaWinner: boolean;
  canSelectConditionBranch: boolean;
  canCancelNode: boolean;
}

export interface WorkflowNodeRuntimeSummary {
  status: WorkflowNodeWorkStatus;
  runtimeHealth: WorkflowNodeWorkView['runtime_health'];
  activeElapsedMs: number | null;
  activeSlow: boolean;
  pendingWorkCount: number;
  startingChildCount: number;
}

export type WorkflowRuntimeAttentionKind = 'waiting' | 'failed';

export interface WorkflowRuntimeAttentionItem {
  nodeId: string;
  nodeType: string;
  status: WorkflowNodeWorkStatus;
  kind: WorkflowRuntimeAttentionKind;
}

export type WorkflowNodeTaskTarget =
  | {
      kind: 'agent-session';
      taskId: string;
      sessionId: string;
    }
  | {
      kind: 'arena';
      taskId: string;
      arenaGroupId: string;
    };

export interface WorkflowRunActionGate {
  canCancel: boolean;
  cancellationPending: boolean;
}

export interface WorkflowSplitProgress {
  total: number;
  completed: number;
  failed: number;
  running: number;
}

export interface WorkflowSplitStage {
  index: number;
  subtasks: WorkflowSplitSubtaskView[];
}

/** Counts accepted subtasks only. A successful summary never completes missing work. */
export function getWorkflowSplitProgress(
  subtasks: WorkflowSplitSubtaskView[]
): WorkflowSplitProgress {
  return {
    total: subtasks.length,
    completed: subtasks.filter((task) => task.state === 'succeeded').length,
    failed: subtasks.filter((task) => task.state === 'failed').length,
    running: subtasks.filter((task) => task.state === 'running').length,
  };
}

export function getWorkflowSplitStages(
  batch: WorkflowSplitBatchView
): WorkflowSplitStage[] {
  const stages = new Map<number, WorkflowSplitSubtaskView[]>();
  for (const task of batch.subtasks) {
    const tasks = stages.get(task.stage_index) ?? [];
    tasks.push(task);
    stages.set(task.stage_index, tasks);
  }
  return [...stages]
    .sort(([left], [right]) => left - right)
    .map(([index, subtasks]) => ({ index, subtasks }));
}

export function getWorkflowSplitBatchesForNode(
  run: WorkflowRunResponse,
  nodeId: string
): WorkflowSplitBatchView[] {
  return run.split_batches.filter(
    (batch) =>
      batch.splitter_node_id === nodeId ||
      batch.summary_node_id === nodeId ||
      batch.subtasks.some((task) => task.node_id === nodeId)
  );
}

export function getWorkflowSplitNodeProgress(
  run: WorkflowRunResponse,
  nodeId: string
): WorkflowSplitProgress | null {
  const subtasks = run.split_batches.flatMap((batch) =>
    batch.splitter_node_id === nodeId
      ? batch.subtasks
      : batch.subtasks.filter((task) => task.node_id === nodeId)
  );
  return subtasks.length ? getWorkflowSplitProgress(subtasks) : null;
}

/** This only enables an explicit submission. The backend checks source freshness and files. */
export function canRequestWorkflowPartialSummary(
  run: WorkflowRunResponse,
  batch: WorkflowSplitBatchView
): boolean {
  return Boolean(
    run.attempt_id &&
      run.status !== 'pending' &&
      run.status !== 'cancelling' &&
      run.status !== 'canceled' &&
      batch.run_id === run.id &&
      batch.summary_node_id &&
      batch.subtasks.some(
        (task) => task.state === 'succeeded' && task.handoff_path
      ) &&
      batch.subtasks.some((task) =>
        ['failed', 'cancelled', 'blocked'].includes(task.state)
      )
  );
}

export function buildWorkflowPartialSummarySubmission(
  run: WorkflowRunResponse,
  batch: WorkflowSplitBatchView,
  requestId: string,
  requirements: string
): WorkflowSubmission | null {
  if (!canRequestWorkflowPartialSummary(run, batch)) return null;
  return {
    request_id: requestId,
    action: 'rework',
    source_run_id: run.id,
    source_batch_id: batch.id,
    delivery_intent: 'partial',
    scope: { type: 'from_nodes', node_ids: [batch.summary_node_id] },
    active_policy: 'after_current',
    input_text: requirements,
    material_paths: [batch.manifest_path],
  };
}

export function getWorkflowRuntimeView(
  run: WorkflowRunResponse,
  options: WorkflowRuntimeViewOptions = {}
): WorkflowRuntimeProjection {
  if (run.runtime_view) {
    return normalizeBackendRuntimeView(run.runtime_view);
  }

  return buildFallbackWorkflowRuntimeView(run, options);
}

export function getWorkflowNodeWork(
  view: WorkflowRuntimeProjection | WorkflowRunRuntimeView,
  nodeId: string | null | undefined
): CanonicalWorkflowNodeWorkView | null {
  if (!nodeId) return null;
  return view.node_work.find((work) => work.node_id === nodeId) ?? null;
}

/**
 * Resolve the detail row for a canonical node projection. Runtime state is
 * matched by orchestration identity first; a legacy process id is never used
 * to identify current work.
 */
export function getWorkflowNodeExecutionForWork(
  run: WorkflowRunResponse,
  work: WorkflowNodeWorkView | null | undefined
): WorkflowNodeExecutionResponse | null {
  if (!work) return null;
  const canonicalWork = work as CanonicalWorkflowNodeWorkView;
  if (canonicalWork.runtime_authority !== 'current') return null;
  // Reuse is a reference to an earlier Run, never a current execution row.
  if (canonicalWork.status === 'reused') return null;

  const orchestrationNodeExecutionId =
    canonicalWork.orchestration_node_execution_id;
  const agentRunId = canonicalWork.active_agent_run_id;
  const latestSlotExecution = selectLatestNodeExecution(
    run.nodes.filter(
      (node) =>
        node.node_id === canonicalWork.node_id &&
        node.iteration === canonicalWork.iteration
    )
  );

  return (
    (orchestrationNodeExecutionId
      ? (run.nodes.find(
          (node) =>
            (node as CanonicalWorkflowNodeExecution)
              .orchestration_node_execution_id === orchestrationNodeExecutionId
        ) ?? null)
      : null) ??
    (agentRunId
      ? (run.nodes.find(
          (node) =>
            (node as CanonicalWorkflowNodeExecution).agent_run_id === agentRunId
        ) ?? null)
      : null) ??
    (canonicalWork.active_execution_id
      ? (run.nodes.find(
          (node) => node.id === canonicalWork.active_execution_id
        ) ?? null)
      : null) ??
    latestSlotExecution ??
    null
  );
}

export function isWorkflowNodeProcessing(
  work: WorkflowNodeWorkView | null | undefined
): boolean {
  return work?.status === 'starting' || work?.status === 'running';
}

export function getWorkflowNodeActionGate(
  work: WorkflowNodeWorkView | null | undefined
): WorkflowNodeActionGate {
  const canonicalWork = work as
    | CanonicalWorkflowNodeWorkView
    | null
    | undefined;
  if (
    canonicalWork?.runtime_authority !== 'current' ||
    canonicalWork.status === 'reused' ||
    canonicalWork.status === 'skipped'
  ) {
    return {
      canOpenSession: false,
      canRetry: false,
      canApprove: false,
      canReject: false,
      canSelectArenaWinner: false,
      canSelectConditionBranch: false,
      canCancelNode: false,
    };
  }
  return {
    canOpenSession: work?.can_open_session ?? false,
    canRetry: work?.can_retry ?? false,
    canApprove: work?.can_approve ?? false,
    canReject: work?.can_reject ?? false,
    canSelectArenaWinner: work?.can_select_arena_winner ?? false,
    canSelectConditionBranch: work?.can_select_condition_branch ?? false,
    canCancelNode: work?.can_cancel_node ?? false,
  };
}

export function getWorkflowNodeRuntimeSummary(
  work: WorkflowNodeWorkView
): WorkflowNodeRuntimeSummary {
  return {
    status: work.status,
    runtimeHealth: work.runtime_health,
    activeElapsedMs: work.active_elapsed_ms,
    activeSlow: work.active_slow,
    pendingWorkCount: work.pending_work_count,
    startingChildCount: work.starting_child_count,
  };
}

/**
 * Project the small set of Nodes that require operator attention. Unknown or
 * degraded fallback work is intentionally omitted so the runtime UI never
 * promotes locally reconstructed state into an actionable notice.
 */
export function getWorkflowRuntimeAttentionItems(
  view: WorkflowRuntimeProjection
): WorkflowRuntimeAttentionItem[] {
  const items: WorkflowRuntimeAttentionItem[] = [];
  for (const work of view.node_work) {
    if (work.runtime_authority !== 'current') continue;

    if (work.status === 'awaiting_human' || work.status === 'awaiting_arena') {
      items.push({
        nodeId: work.node_id,
        nodeType: work.node_type,
        status: work.status,
        kind: 'waiting',
      });
      continue;
    }

    if (work.status === 'failed') {
      items.push({
        nodeId: work.node_id,
        nodeType: work.node_type,
        status: work.status,
        kind: 'failed',
      });
    }
  }

  return items.sort((left, right) => {
    if (left.kind === right.kind) return 0;
    return left.kind === 'waiting' ? -1 : 1;
  });
}

/**
 * Resolve Task-owned runtime navigation. Session or Arena identities without
 * a canonical Task binding are deliberately not exposed as Node deep links.
 */
export function getWorkflowNodeExecutionTarget(
  execution: WorkflowNodeExecutionResponse | null | undefined,
  work: WorkflowNodeWorkView | null | undefined
): WorkflowNodeTaskTarget | null {
  const canonicalWork = work as
    | CanonicalWorkflowNodeWorkView
    | null
    | undefined;
  if (
    !execution?.execution_id ||
    canonicalWork?.runtime_authority !== 'current' ||
    canonicalWork.status === 'reused' ||
    canonicalWork.status === 'skipped'
  ) {
    return null;
  }

  if (
    (execution.node_type === 'agent' ||
      execution.node_type === 'task_splitter') &&
    execution.session_id &&
    getWorkflowNodeActionGate(work).canOpenSession
  ) {
    return {
      kind: 'agent-session',
      taskId: execution.execution_id,
      sessionId: execution.session_id,
    };
  }

  if (execution.node_type === 'arena' && execution.arena_group_id) {
    return {
      kind: 'arena',
      taskId: execution.execution_id,
      arenaGroupId: execution.arena_group_id,
    };
  }

  return null;
}

export function getWorkflowRunActionGate(
  run: WorkflowRunResponse
): WorkflowRunActionGate {
  return {
    canCancel:
      run.status === 'pending' ||
      run.status === 'running' ||
      run.status === 'awaiting_human' ||
      run.status === 'awaiting_arena',
    cancellationPending: run.status === 'cancelling',
  };
}

export function getDefaultWorkflowRuntimeNodeId(
  view: WorkflowRuntimeProjection | WorkflowRunRuntimeView
): string | null {
  const waiting = view.node_work.find((work) =>
    isWaitingWorkflowNodeWorkStatus(work.status)
  );
  if (waiting) return waiting.node_id;

  const failed = view.node_work.find((work) => work.status === 'failed');
  if (failed) return failed.node_id;

  const active = view.node_work.find((work) =>
    isActiveWorkflowNodeWorkStatus(work.status)
  );
  if (active) return active.node_id;

  return view.node_work[0]?.node_id ?? null;
}

function buildFallbackWorkflowRuntimeView(
  run: WorkflowRunResponse,
  options: WorkflowRuntimeViewOptions
): WorkflowRuntimeProjection {
  const nowMs = options.nowMs ?? Date.now();
  const activeSlowThresholdMs =
    options.activeSlowThresholdMs ?? WORKFLOW_NODE_ACTIVE_SLOW_THRESHOLD_MS;
  const grouped = new Map<string, WorkflowNodeExecutionResponse[]>();
  const orderedNodeIds: string[] = [];

  for (const node of run.nodes) {
    if (!grouped.has(node.node_id)) {
      orderedNodeIds.push(node.node_id);
      grouped.set(node.node_id, []);
    }
    grouped.get(node.node_id)?.push(node);
  }

  const nodeWork = orderedNodeIds
    .map((nodeId) => {
      const executions = grouped.get(nodeId) ?? [];
      const current = selectLatestNodeExecution(executions);
      return current
        ? buildFallbackNodeWorkView(current, executions, {
            nowMs,
            activeSlowThresholdMs,
          })
        : null;
    })
    .filter((work): work is WorkflowNodeWorkView => Boolean(work));

  return {
    run_id: run.id,
    status: run.status,
    active_node_count: nodeWork.filter(isActiveWorkflowNodeWork).length,
    pending_node_count: nodeWork.filter((work) => work.status === 'pending')
      .length,
    waiting_node_count: nodeWork.filter(isWaitingWorkflowNodeWork).length,
    failed_node_count: nodeWork.filter((work) => work.status === 'failed')
      .length,
    completed_node_count: nodeWork.filter((work) => work.status === 'succeeded')
      .length,
    reused_node_count: 0,
    skipped_node_count: nodeWork.filter((work) => work.status === 'skipped')
      .length,
    node_work: nodeWork,
    authority: 'unknown',
  };
}

function normalizeBackendRuntimeView(
  view: WorkflowRunRuntimeView
): WorkflowRuntimeProjection {
  const nodeWork = view.node_work.map((work) => {
    const canonicalWork = work as CanonicalWorkflowNodeWorkView;
    const projectionStatus = canonicalWork.projection_status ?? null;
    const hasCanonicalIdentity = Boolean(
      canonicalWork.orchestration_node_execution_id ||
        canonicalWork.active_agent_run_id
    );
    const runtimeAuthority: WorkflowRuntimeAuthority =
      projectionStatus === 'projection_degraded' ||
      projectionStatus === 'rebuilding' ||
      (canonicalWork.runtime_health as string) === 'projection_degraded'
        ? 'degraded'
        : hasCanonicalIdentity ||
            !['agent', 'task_splitter'].includes(canonicalWork.node_type) ||
            canonicalWork.status === 'reused' ||
            canonicalWork.status === 'skipped'
          ? 'current'
          : 'unknown';

    return {
      ...canonicalWork,
      runtime_authority: runtimeAuthority,
    };
  });

  const authority = nodeWork.some(
    (work) => work.runtime_authority === 'degraded'
  )
    ? 'degraded'
    : nodeWork.some((work) => work.runtime_authority === 'unknown')
      ? 'unknown'
      : 'current';

  return { ...view, node_work: nodeWork, authority };
}

function buildFallbackNodeWorkView(
  current: WorkflowNodeExecutionResponse,
  executions: WorkflowNodeExecutionResponse[],
  {
    nowMs,
    activeSlowThresholdMs,
  }: {
    nowMs: number;
    activeSlowThresholdMs: number;
  }
): CanonicalWorkflowNodeWorkView {
  const canonicalCurrent = current as CanonicalWorkflowNodeExecution;
  const status = workflowNodeWorkStatus(canonicalCurrent);
  const active = isActiveWorkflowNodeWorkStatus(status);
  const waiting = isWaitingWorkflowNodeWorkStatus(status);
  const activeElapsedMs = active
    ? calculateElapsedMs(current.started_at, nowMs)
    : null;
  const activeSlow =
    activeElapsedMs !== null && activeElapsedMs >= activeSlowThresholdMs;

  return {
    node_id: current.node_id,
    node_type: current.node_type,
    iteration: current.iteration,
    status,
    pending_work_count: executions.filter((node) => node.status === 'pending')
      .length,
    starting_child_count: executions.filter(
      (node) =>
        node.status === 'running' &&
        !(
          (node as CanonicalWorkflowNodeExecution).agent_run_id ||
          (node as CanonicalWorkflowNodeExecution)
            .orchestration_node_execution_id
        )
    ).length,
    active_execution_id: active || waiting ? current.id : null,
    active_session_id: current.session_id,
    orchestration_node_execution_id:
      canonicalCurrent.orchestration_node_execution_id ?? null,
    active_agent_run_id: canonicalCurrent.agent_run_id ?? null,
    projection_status: canonicalCurrent.projection_status ?? null,
    active_started_at: active || waiting ? current.started_at : null,
    active_elapsed_ms: activeElapsedMs,
    active_slow: activeSlow,
    active_slow_threshold_ms: activeSlowThresholdMs,
    runtime_health: 'unknown',
    can_open_session: false,
    can_retry: false,
    can_approve: false,
    can_reject: false,
    can_select_arena_winner: false,
    can_select_condition_branch: false,
    can_cancel_node: false,
    reused_results: [],
    runtime_authority: 'unknown',
  };
}

function selectLatestNodeExecution(
  executions: WorkflowNodeExecutionResponse[]
): WorkflowNodeExecutionResponse | null {
  let latest: WorkflowNodeExecutionResponse | null = null;

  for (const execution of executions) {
    if (!latest || isAfterNodeExecution(execution, latest)) {
      latest = execution;
    }
  }

  return latest;
}

function isAfterNodeExecution(
  next: WorkflowNodeExecutionResponse,
  current: WorkflowNodeExecutionResponse
): boolean {
  if (next.iteration !== current.iteration) {
    return next.iteration > current.iteration;
  }

  return next.updated_at > current.updated_at;
}

function workflowNodeWorkStatus(
  node: CanonicalWorkflowNodeExecution
): WorkflowNodeWorkStatus {
  if (
    node.status === 'running' &&
    !node.agent_run_id &&
    !node.orchestration_node_execution_id
  ) {
    return 'starting';
  }
  return node.status;
}

function calculateElapsedMs(startedAt: string | null, nowMs: number) {
  if (!startedAt) return null;
  const startedMs = new Date(startedAt).getTime();
  if (Number.isNaN(startedMs)) return null;
  return Math.max(0, Math.floor(nowMs - startedMs));
}

function isActiveWorkflowNodeWork(work: WorkflowNodeWorkView): boolean {
  return isActiveWorkflowNodeWorkStatus(work.status);
}

function isActiveWorkflowNodeWorkStatus(
  status: WorkflowNodeWorkStatus
): boolean {
  return status === 'starting' || status === 'running';
}

function isWaitingWorkflowNodeWork(work: WorkflowNodeWorkView): boolean {
  return isWaitingWorkflowNodeWorkStatus(work.status);
}

function isWaitingWorkflowNodeWorkStatus(
  status: WorkflowNodeWorkStatus | NodeExecutionStatus
): boolean {
  return status === 'awaiting_human' || status === 'awaiting_arena';
}
