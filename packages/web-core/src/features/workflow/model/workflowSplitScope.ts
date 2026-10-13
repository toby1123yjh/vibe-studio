import type { WorkflowGraph } from './workflowGraph';

export interface WorkflowSplitScope {
  splitterNodeId: string;
  candidateNodeIds: string[];
  summaryNodeId: string;
}

export type WorkflowSplitScopeIssueCode =
  | 'missingExecutors'
  | 'invalidExecutor'
  | 'competingExecutorInputs'
  | 'missingSummary'
  | 'differentSummaries'
  | 'invalidSummary'
  | 'competingSummaryInputs'
  | 'overlappingScopes';

export interface WorkflowSplitScopeIssue {
  nodeId: string;
  code: WorkflowSplitScopeIssueCode;
}

/** Mirrors the fixed graph boundary validated by workflow::dispatch_scope. */
export function getWorkflowSplitScopes(graph: WorkflowGraph): {
  scopes: WorkflowSplitScope[];
  issues: WorkflowSplitScopeIssue[];
} {
  const scopes: WorkflowSplitScope[] = [];
  const issues: WorkflowSplitScopeIssue[] = [];
  const owners = new Map<string, string>();
  for (const node of graph.nodes) {
    if (node.type !== 'task_splitter') continue;
    const result = resolveSplitScope(graph, node.id);
    if ('code' in result) {
      issues.push(result);
      continue;
    }
    const scopeNodes = [...result.candidateNodeIds, result.summaryNodeId];
    const overlaps = scopeNodes.some((id) => owners.has(id));
    if (overlaps) {
      issues.push({ nodeId: node.id, code: 'overlappingScopes' });
      continue;
    }
    for (const id of scopeNodes) owners.set(id, node.id);
    scopes.push(result);
  }
  return { scopes, issues };
}

function resolveSplitScope(
  graph: WorkflowGraph,
  splitterNodeId: string
): WorkflowSplitScope | WorkflowSplitScopeIssue {
  const fail = (code: WorkflowSplitScopeIssueCode) => ({
    nodeId: splitterNodeId,
    code,
  });
  const outgoing = graph.edges.filter((e) => e.source === splitterNodeId);
  if (!outgoing.length) return fail('missingExecutors');
  const nodeById = new Map(graph.nodes.map((node) => [node.id, node]));
  const candidateNodeIds: string[] = [];
  let summaryNodeId: string | null = null;
  for (const edge of outgoing) {
    const candidate = nodeById.get(edge.target);
    if (
      edge.type !== 'default' ||
      candidate?.type !== 'agent' ||
      candidateNodeIds.includes(edge.target)
    ) {
      return fail('invalidExecutor');
    }
    if (graph.edges.filter((e) => e.target === candidate.id).length !== 1) {
      return fail('competingExecutorInputs');
    }
    const exits = graph.edges.filter((e) => e.source === candidate.id);
    if (exits.length !== 1 || exits[0].type !== 'default') {
      return fail('missingSummary');
    }
    if (summaryNodeId && summaryNodeId !== exits[0].target) {
      return fail('differentSummaries');
    }
    summaryNodeId = exits[0].target;
    candidateNodeIds.push(candidate.id);
  }
  const summary = summaryNodeId ? nodeById.get(summaryNodeId) : null;
  if (
    !summary ||
    !['agent', 'transform', 'end'].includes(summary.type) ||
    candidateNodeIds.includes(summary.id)
  ) {
    return fail('invalidSummary');
  }
  if (
    graph.edges
      .filter((e) => e.target === summary.id)
      .some((e) => !candidateNodeIds.includes(e.source) || e.type !== 'default')
  ) {
    return fail('competingSummaryInputs');
  }
  return { splitterNodeId, candidateNodeIds, summaryNodeId: summary.id };
}
