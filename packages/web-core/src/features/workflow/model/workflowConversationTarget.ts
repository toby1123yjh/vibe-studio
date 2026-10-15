import type {
  WorkflowContextFileView,
  WorkflowFileContextView,
  WorkflowNodeExecutionResponse,
  WorkflowReuseView,
  WorkflowRunResponse,
  WorkflowSplitSubtaskView,
} from 'shared/types';
import { buildWorkspaceSessionHref } from '../../../shared/lib/routes/workspaceRoutes';
import {
  getWorkflowNodeExecutionForWork,
  getWorkflowNodeExecutionTarget,
  getWorkflowNodeWork,
  type WorkflowRuntimeProjection,
} from './workflowRuntimeView';

/** View identity only. Runtime and the native Session remain the state owners. */
export interface WorkflowConversationTarget {
  key: string;
  kind: 'node' | 'subtask' | 'candidate' | 'main' | 'source';
  runId: string;
  nodeId: string | null;
  nodeExecutionId: string | null;
  workspaceId: string | null;
  sessionId: string | null;
  title: string;
  status: string;
  readOnly: boolean;
  /** A cloned member needs the validated original execution binding. */
  resolveSource?: boolean;
  arenaGroupId?: string | null;
  candidateId?: string;
}

export function workflowConversationScopeKey(
  hostId: string | null,
  target: WorkflowConversationTarget
): string {
  return JSON.stringify([
    hostId,
    target.runId,
    target.key,
    target.nodeExecutionId,
    target.workspaceId,
    target.sessionId,
  ]);
}

export function buildWorkflowConversationHref({
  hostId,
  projectId,
  taskId,
  workspaceId,
  sessionId,
}: {
  hostId: string | null;
  projectId: string;
  taskId: string;
  workspaceId: string | null;
  sessionId: string | null;
}): string | null {
  if (!workspaceId || !sessionId) return null;
  const hostPrefix = hostId ? `/hosts/${encodeURIComponent(hostId)}` : '';
  return buildWorkspaceSessionHref(
    `${hostPrefix}/projects/${encodeURIComponent(projectId)}/tasks/${encodeURIComponent(taskId)}/workspaces/${encodeURIComponent(workspaceId)}`,
    sessionId
  );
}

export function workflowSubtaskConversation(
  run: WorkflowRunResponse,
  batchId: string,
  task: WorkflowSplitSubtaskView
): WorkflowConversationTarget {
  const execution = run.nodes.find(
    (item) =>
      item.id === task.node_execution_id && item.node_id === task.node_id
  );
  const resolveSource =
    Boolean(task.node_execution_id) &&
    (!execution || execution.run_id !== run.id);
  // Both identities must agree; another work item's Session is never a fallback.
  const sessionId =
    execution &&
    !resolveSource &&
    (!execution.session_id || execution.session_id === task.session_id)
      ? task.session_id
      : null;
  return {
    key: `subtask:${batchId}:${task.id}`,
    kind: 'subtask',
    runId: run.id,
    nodeId: task.node_id,
    nodeExecutionId: execution?.id ?? task.node_execution_id,
    workspaceId: resolveSource ? null : run.workspace_id,
    sessionId: resolveSource ? task.session_id : sessionId,
    title: task.title,
    status: task.state,
    readOnly: resolveSource,
    resolveSource,
  };
}

export function getWorkflowNodeConversations(
  run: WorkflowRunResponse,
  runtime: WorkflowRuntimeProjection,
  nodeId: string,
  title: string
): WorkflowConversationTarget[] {
  const work = getWorkflowNodeWork(runtime, nodeId);
  const tasks = run.split_batches.flatMap((batch) =>
    batch.subtasks
      .filter((task) => task.node_id === nodeId)
      .map((task) => ({ batchId: batch.id, task }))
  );
  if (tasks.length) {
    return tasks
      .sort(
        (left, right) =>
          left.task.stage_index - right.task.stage_index ||
          left.task.id.localeCompare(right.task.id)
      )
      .map(({ batchId, task }) =>
        workflowSubtaskConversation(run, batchId, task)
      );
  }
  if (work?.status === 'reused' || work?.status === 'skipped') return [];
  const execution = getWorkflowNodeExecutionForWork(run, work);
  const executionTarget = getWorkflowNodeExecutionTarget(execution, work);
  return [
    {
      key: `node:${nodeId}`,
      kind: 'node',
      runId: run.id,
      nodeId,
      nodeExecutionId: execution?.id ?? null,
      workspaceId: run.workspace_id,
      sessionId:
        executionTarget?.kind === 'agent-session'
          ? executionTarget.sessionId
          : null,
      title,
      status: work?.status ?? 'pending',
      readOnly: false,
    },
  ];
}

/** Resolve only the exact member validated by the read-only API projection. */
export function resolveWorkflowConversationContext(
  target: WorkflowConversationTarget,
  context: WorkflowFileContextView | undefined
): WorkflowConversationTarget {
  if (!target.resolveSource) return target;
  const execution = context?.execution;
  if (
    !context ||
    context.run_id !== target.runId ||
    context.node_execution_id !== target.nodeExecutionId ||
    !execution ||
    execution.node_execution_id !== target.nodeExecutionId ||
    execution.node_id !== target.nodeId ||
    (target.sessionId && execution.session_id !== target.sessionId)
  ) {
    return { ...target, workspaceId: null, sessionId: null, readOnly: true };
  }
  return {
    ...target,
    kind: 'source',
    runId: execution.run_id,
    workspaceId: execution.workspace_id,
    sessionId: execution.session_id,
    status: execution.status,
    readOnly: true,
    resolveSource: false,
  };
}

/** A retained selection wins over polling changes to active work. */
export function selectWorkflowConversation(
  targets: WorkflowConversationTarget[],
  previousKey?: string | null
): WorkflowConversationTarget | null {
  return (
    targets.find((target) => target.key === previousKey) ??
    targets.find((target) =>
      ['starting', 'running', 'awaiting_human', 'cancelling'].includes(
        target.status
      )
    ) ??
    targets[0] ??
    null
  );
}

export function workflowSourceConversation(
  source: WorkflowReuseView,
  title: string
): WorkflowConversationTarget {
  return {
    key: `source:${source.source_run_id}:${source.source_node_execution_id}`,
    kind: 'source',
    runId: source.source_run_id,
    nodeId: source.node_id,
    nodeExecutionId: source.source_node_execution_id,
    workspaceId: null,
    sessionId: null,
    title,
    status: 'succeeded',
    readOnly: true,
  };
}

export function resolveWorkflowSourceConversation(
  target: WorkflowConversationTarget,
  sourceRun: WorkflowRunResponse | undefined
): WorkflowConversationTarget {
  if (!sourceRun || sourceRun.id !== target.runId) return target;
  const execution = sourceRun.nodes.find(
    (item) =>
      item.id === target.nodeExecutionId && item.node_id === target.nodeId
  );
  return {
    ...target,
    workspaceId: execution ? sourceRun.workspace_id : null,
    sessionId: execution?.session_id ?? null,
    status: execution?.status ?? target.status,
    readOnly: true,
  };
}

export function getWorkflowOutputCount(
  files: WorkflowContextFileView[]
): number {
  return new Set(
    files
      .filter((file) => file.role === 'output')
      .map((file) => `${file.source_workspace_id}:${file.path}`)
  ).size;
}

export function workflowCandidateConversation({
  run,
  execution,
  candidateId,
  workspaceId,
  sessionId,
  title,
  status,
}: {
  run: WorkflowRunResponse;
  execution: WorkflowNodeExecutionResponse;
  candidateId: string;
  workspaceId: string;
  sessionId: string | null;
  title: string;
  status: string;
}): WorkflowConversationTarget {
  return {
    key: `candidate:${execution.id}:${candidateId}`,
    kind: 'candidate',
    runId: run.id,
    nodeId: execution.node_id,
    nodeExecutionId: execution.id,
    workspaceId,
    sessionId,
    title,
    status,
    readOnly: false,
    arenaGroupId: execution.arena_group_id,
    candidateId,
  };
}
