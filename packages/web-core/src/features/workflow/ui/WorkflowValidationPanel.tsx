import {
  WORKFLOW_GRAPH_VERSION,
  type WorkflowConditionBranch,
  type WorkflowGraph,
} from '../model/workflowGraph';
import { AlertCircle, CheckCircle2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { BaseCodingAgent } from 'shared/types';
import {
  getWorkflowSplitScopes,
  type WorkflowSplitScopeIssueCode,
} from '../model/workflowSplitScope';

const SPLIT_SCOPE_MESSAGES: Record<WorkflowSplitScopeIssueCode, string> = {
  missingExecutors: 'Connect the splitter to at least one Agent executor.',
  invalidExecutor:
    'The splitter can only connect directly to distinct Agent executors.',
  competingExecutorInputs:
    'A split executor cannot have another incoming trigger.',
  missingSummary:
    'Each split executor needs one default connection to the summary exit.',
  differentSummaries:
    'All split executors must connect to the same summary exit.',
  invalidSummary:
    'The summary exit must be a separate Agent, Transform, or End node.',
  competingSummaryInputs:
    'The summary exit cannot have incoming triggers outside this split scope.',
  overlappingScopes: 'Split scopes cannot share an executor or summary exit.',
};

export interface ValidationIssue {
  type: 'error' | 'warning';
  nodeId?: string;
  message: string;
  splitScopeCode?: WorkflowSplitScopeIssueCode;
}

export interface WorkflowValidationPanelProps {
  graph: WorkflowGraph | null;
}

export interface WorkflowValidationOptions {
  includeRunReadiness?: boolean;
}

export function validateWorkflowGraph(
  graph: WorkflowGraph | null,
  options: WorkflowValidationOptions = {}
): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  if (!graph) return issues;
  const includeRunReadiness = options.includeRunReadiness ?? true;

  if (graph.version < 1 || graph.version > WORKFLOW_GRAPH_VERSION) {
    issues.push({ type: 'error', message: 'Unsupported graph version' });
  }

  const nodes = graph.nodes;
  const edges = graph.edges;

  const startNodes = nodes.filter((n) => n.type === 'start');
  if (startNodes.length !== 1) {
    issues.push({
      type: 'error',
      message: `Expected exactly 1 Start node, found ${startNodes.length}`,
    });
  }

  const endNodes = nodes.filter((n) => n.type === 'end');
  if (endNodes.length === 0) {
    issues.push({
      type: 'error',
      message: 'Workflow must have at least one End node',
    });
  }

  const nodeIds = new Set<string>();
  for (const node of nodes) {
    if (nodeIds.has(node.id)) {
      issues.push({
        type: 'error',
        message: `Duplicate node ID found: ${node.id}`,
      });
    }
    nodeIds.add(node.id);
  }

  for (const edge of edges) {
    if (edge.source === edge.target) {
      issues.push({
        type: 'error',
        nodeId: edge.source,
        message: `Self-edge found on node ${edge.source}`,
      });
    }
    if (!nodeIds.has(edge.source)) {
      issues.push({
        type: 'error',
        nodeId: edge.source,
        message: `Edge source node missing: ${edge.source}`,
      });
    }
    if (!nodeIds.has(edge.target)) {
      issues.push({
        type: 'error',
        nodeId: edge.target,
        message: `Edge target node missing: ${edge.target}`,
      });
    }
  }

  validateConditionStructure(graph, issues);
  for (const issue of getWorkflowSplitScopes(graph).issues) {
    issues.push({
      type: 'error',
      nodeId: issue.nodeId,
      splitScopeCode: issue.code,
      message: SPLIT_SCOPE_MESSAGES[issue.code],
    });
  }

  const adjacency = buildAdjacency(graph);
  const cycleNodeId = findCycleNode(adjacency);
  if (cycleNodeId) {
    issues.push({
      type: 'error',
      nodeId: cycleNodeId,
      message: `Workflow contains a cycle at ${cycleNodeId}`,
    });
  }

  if (startNodes.length === 1) {
    const reachable = findReachableNodes(startNodes[0].id, adjacency);
    for (const node of nodes) {
      if (node.type !== 'start' && !reachable.has(node.id)) {
        issues.push({
          type: 'error',
          nodeId: node.id,
          message: `Unreachable node: ${node.id}`,
        });
      }
    }
  }

  if (includeRunReadiness) {
    validateConditionRunReadiness(graph, issues);
  }

  return issues;
}

function getConditionOutgoingTargets(
  graph: WorkflowGraph,
  conditionNodeId: string
): string[] {
  return graph.edges
    .filter((edge) => edge.source === conditionNodeId)
    .map((edge) => edge.target);
}

function validateConditionStructure(
  graph: WorkflowGraph,
  issues: ValidationIssue[]
) {
  for (const node of graph.nodes) {
    if (node.type !== 'condition') continue;

    const seenTargets = new Set<string>();
    for (const target of getConditionOutgoingTargets(graph, node.id)) {
      if (seenTargets.has(target)) {
        issues.push({
          type: 'error',
          nodeId: node.id,
          message: `Condition node ${node.id} has duplicate outgoing target: ${target}`,
        });
      }
      seenTargets.add(target);
    }
  }
}

function validateConditionRunReadiness(
  graph: WorkflowGraph,
  issues: ValidationIssue[]
) {
  const conditionNodes = graph.nodes.filter(
    (node) => node.type === 'condition'
  );
  if (conditionNodes.length === 0) return;

  const hasRouterConfig = hasRouterExecutorConfig(graph.router_executor_config);
  if (!hasRouterConfig) {
    issues.push({
      type: 'error',
      message: 'Workflow with Condition nodes requires a router agent',
    });
  }

  for (const node of conditionNodes) {
    const outgoingTargets = new Set(
      getConditionOutgoingTargets(graph, node.id)
    );
    const branchTargets = new Set<string>();
    const branches = node.data.branches ?? [];

    for (const branch of branches) {
      validateConditionBranch(
        node.id,
        branch,
        outgoingTargets,
        branchTargets,
        issues
      );
    }

    for (const target of outgoingTargets) {
      if (!branchTargets.has(target)) {
        issues.push({
          type: 'error',
          nodeId: node.id,
          message: `Condition node ${node.id} is missing branch config for ${target}`,
        });
      }
    }
  }
}

function hasRouterExecutorConfig(config: unknown): boolean {
  if (!config || typeof config !== 'object') return false;
  const executor = (config as { executor?: unknown }).executor;
  if (typeof executor !== 'string') return false;
  const normalized = executor.trim().replaceAll('-', '_').toUpperCase();
  return Object.values(BaseCodingAgent).includes(normalized as BaseCodingAgent);
}

function validateConditionBranch(
  nodeId: string,
  branch: WorkflowConditionBranch,
  outgoingTargets: Set<string>,
  branchTargets: Set<string>,
  issues: ValidationIssue[]
) {
  const targetNodeId = branch.target_node_id;
  if (!targetNodeId) {
    issues.push({
      type: 'error',
      nodeId,
      message: `Condition node ${nodeId} has a branch without target`,
    });
    return;
  }

  if (branchTargets.has(targetNodeId)) {
    issues.push({
      type: 'error',
      nodeId,
      message: `Condition node ${nodeId} has duplicate branch target: ${targetNodeId}`,
    });
  }
  branchTargets.add(targetNodeId);

  if (!outgoingTargets.has(targetNodeId)) {
    issues.push({
      type: 'error',
      nodeId,
      message: `Condition node ${nodeId} has stale branch target: ${targetNodeId}`,
    });
  }

  if (!(branch.condition ?? '').trim()) {
    issues.push({
      type: 'error',
      nodeId,
      message: `Condition node ${nodeId} has empty branch condition for ${targetNodeId}`,
    });
  }
}

function buildAdjacency(graph: WorkflowGraph): Map<string, string[]> {
  const nodeIds = new Set(graph.nodes.map((node) => node.id));
  const adjacency = new Map<string, string[]>();
  for (const node of graph.nodes) {
    adjacency.set(node.id, []);
  }

  for (const edge of graph.edges) {
    if (!nodeIds.has(edge.source) || !nodeIds.has(edge.target)) {
      continue;
    }
    adjacency.get(edge.source)?.push(edge.target);
  }

  return adjacency;
}

function findCycleNode(adjacency: Map<string, string[]>): string | null {
  const states = new Map<string, 'visiting' | 'visited'>();

  const visit = (nodeId: string): string | null => {
    const state = states.get(nodeId);
    if (state === 'visiting') return nodeId;
    if (state === 'visited') return null;

    states.set(nodeId, 'visiting');
    for (const target of adjacency.get(nodeId) ?? []) {
      const cycleNodeId = visit(target);
      if (cycleNodeId) return cycleNodeId;
    }
    states.set(nodeId, 'visited');
    return null;
  };

  for (const nodeId of adjacency.keys()) {
    const cycleNodeId = visit(nodeId);
    if (cycleNodeId) return cycleNodeId;
  }

  return null;
}

function findReachableNodes(
  startNodeId: string,
  adjacency: Map<string, string[]>
): Set<string> {
  const reachable = new Set<string>();
  const queue = [startNodeId];

  while (queue.length > 0) {
    const nodeId = queue.shift();
    if (!nodeId || reachable.has(nodeId)) continue;
    reachable.add(nodeId);
    queue.push(...(adjacency.get(nodeId) ?? []));
  }

  return reachable;
}

export function WorkflowValidationPanel({
  graph,
}: WorkflowValidationPanelProps) {
  const { t } = useTranslation('common');
  const issues = validateWorkflowGraph(graph);

  return (
    <div className="max-h-48 overflow-y-auto border-t border-secondary bg-panel p-4 text-sm">
      <h3 className="mb-2 font-semibold text-high">
        {t('workflow.editor.validation')}
      </h3>
      {issues.length === 0 ? (
        <div className="flex items-center gap-2 text-success">
          <CheckCircle2 className="h-4 w-4" />
          <span>{t('workflow.editor.graphValid')}</span>
        </div>
      ) : (
        <ul className="flex flex-col gap-2">
          {issues.map((issue, i) => (
            <li
              key={i}
              className={`flex items-start gap-2 ${
                issue.type === 'error' ? 'text-error' : 'text-brand'
              }`}
            >
              <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
              <span>
                {issue.splitScopeCode
                  ? t(`workflow.split.scopeErrors.${issue.splitScopeCode}`)
                  : issue.message}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
