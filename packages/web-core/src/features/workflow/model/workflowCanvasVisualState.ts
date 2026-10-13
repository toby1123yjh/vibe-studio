import type { TFunction } from 'i18next';
import type {
  NodeExecutionStatus,
  WorkflowNodeExecutionResponse,
  WorkflowNodeWorkStatus,
} from 'shared/types';
import type {
  WorkflowGraph,
  WorkflowNodeData,
  WorkflowNodeKind,
} from './workflowGraph';

export type WorkflowCanvasNodeState =
  | 'draft'
  | 'configured'
  | 'pending'
  | 'running'
  | 'succeeded'
  | 'failed'
  | 'waiting'
  | 'reused'
  | 'skipped';

export type WorkflowCanvasEdgeState =
  | 'idle'
  | 'running'
  | 'succeeded'
  | 'failed'
  | 'waiting'
  | 'reused'
  | 'skipped';

export type WorkflowNodeExecutionStatusMap = Record<
  string,
  NodeExecutionStatus
>;

const NODE_STATE_LABELS: Record<WorkflowCanvasNodeState, string> = {
  draft: 'Draft',
  configured: 'Configured',
  pending: 'Pending',
  running: 'Running',
  succeeded: 'Done',
  failed: 'Failed',
  waiting: 'Waiting',
  reused: 'Reused',
  skipped: 'Skipped',
};

function hasConfiguredAgentData(data: WorkflowNodeData): boolean {
  const prompt = data.prompt_template;
  const executorConfig = data.executor_config;
  return (
    (typeof prompt === 'string' && prompt.trim().length > 0) ||
    executorConfig != null
  );
}

function isAfterExecution(
  next: WorkflowNodeExecutionResponse,
  current: WorkflowNodeExecutionResponse
): boolean {
  if (next.iteration !== current.iteration) {
    return next.iteration > current.iteration;
  }

  return next.updated_at > current.updated_at;
}

export function buildWorkflowNodeExecutionStatusMap(
  executions: WorkflowNodeExecutionResponse[] | null | undefined
): WorkflowNodeExecutionStatusMap {
  const latestByNodeId = new Map<string, WorkflowNodeExecutionResponse>();

  for (const execution of executions ?? []) {
    const current = latestByNodeId.get(execution.node_id);
    if (!current || isAfterExecution(execution, current)) {
      latestByNodeId.set(execution.node_id, execution);
    }
  }

  return Object.fromEntries(
    Array.from(latestByNodeId.entries()).map(([nodeId, execution]) => [
      nodeId,
      execution.status,
    ])
  );
}

export function getWorkflowCanvasNodeState({
  data,
  executionStatus,
  nodeType,
}: {
  data: WorkflowNodeData;
  executionStatus?: NodeExecutionStatus | WorkflowNodeWorkStatus;
  nodeType: WorkflowNodeKind;
}): WorkflowCanvasNodeState {
  switch (executionStatus) {
    case 'pending':
      return 'pending';
    case 'running':
    case 'starting':
    case 'cancelling':
      return 'running';
    case 'succeeded':
      return 'succeeded';
    case 'reused':
      return 'reused';
    case 'failed':
      return 'failed';
    case 'awaiting_human':
    case 'awaiting_arena':
      return 'waiting';
    case 'skipped':
    case 'cancelled':
      return 'skipped';
    default:
      return (nodeType === 'agent' || nodeType === 'task_splitter') &&
        !hasConfiguredAgentData(data)
        ? 'draft'
        : 'configured';
  }
}

export function getWorkflowCanvasNodeStateLabel(
  state: WorkflowCanvasNodeState,
  t?: TFunction<'common'>
): string {
  return t
    ? t(`workflow.canvasState.${state}`, {
        defaultValue: NODE_STATE_LABELS[state],
      })
    : NODE_STATE_LABELS[state];
}

export function getWorkflowCanvasEdgeState(
  sourceStatus: NodeExecutionStatus | WorkflowNodeWorkStatus | undefined,
  targetStatus?: NodeExecutionStatus | WorkflowNodeWorkStatus
): WorkflowCanvasEdgeState {
  if (sourceStatus === 'failed' || targetStatus === 'failed') return 'failed';
  if (
    sourceStatus === 'running' ||
    sourceStatus === 'starting' ||
    sourceStatus === 'cancelling' ||
    targetStatus === 'running' ||
    targetStatus === 'starting' ||
    targetStatus === 'cancelling'
  ) {
    return 'running';
  }
  if (
    sourceStatus === 'awaiting_human' ||
    sourceStatus === 'awaiting_arena' ||
    targetStatus === 'awaiting_human' ||
    targetStatus === 'awaiting_arena'
  ) {
    return 'waiting';
  }
  if (
    sourceStatus === 'skipped' ||
    sourceStatus === 'cancelled' ||
    targetStatus === 'skipped' ||
    targetStatus === 'cancelled'
  ) {
    return 'skipped';
  }
  if (sourceStatus === 'succeeded' || targetStatus === 'succeeded') {
    return 'succeeded';
  }
  if (sourceStatus === 'reused' || targetStatus === 'reused') return 'reused';
  return 'idle';
}

export function buildWorkflowEdgeStateMap(
  graph: WorkflowGraph,
  nodeStatuses: WorkflowNodeExecutionStatusMap | null | undefined
): Record<string, WorkflowCanvasEdgeState> {
  return Object.fromEntries(
    graph.edges.map((edge) => [
      edge.id,
      getWorkflowCanvasEdgeState(
        nodeStatuses?.[edge.source],
        nodeStatuses?.[edge.target]
      ),
    ])
  );
}
