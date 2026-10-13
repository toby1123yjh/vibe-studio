import {
  WORKFLOW_GRAPH_VERSION,
  createWorkflowEdge,
  createWorkflowNode,
  type WorkflowEdge,
  type WorkflowEdgeKind,
  type WorkflowGraph,
  type WorkflowNode,
  type WorkflowNodeData,
  type WorkflowNodeKind,
  type WorkflowNodePosition,
} from './workflowGraph';
import { coerceWorkflowNodeExecutorConfig } from './workflowAgentNodeDraft';
import {
  getWorkflowSplitScopes,
  type WorkflowSplitScopeIssueCode,
} from './workflowSplitScope';

export const WORKFLOW_SEMANTIC_HANDLE_IDS = {
  input: 'input',
  default: 'default',
  winner: 'winner',
  approve: 'approve',
  reject: 'reject',
} as const;

const LEGACY_POSITIONAL_HANDLE_IDS = new Set([
  'port-left',
  'port-top',
  'port-right',
  'port-bottom',
  'input-left',
  'input-top',
  'input-right',
  'input-bottom',
  'output-left',
  'output-top',
  'output-right',
  'output-bottom',
]);

export type WorkflowSemanticHandleKind =
  | 'input'
  | 'default'
  | 'winner'
  | 'approve'
  | 'reject'
  | 'condition_branch';

export interface WorkflowSemanticHandle {
  id: string;
  kind: WorkflowSemanticHandleKind;
  label: string;
  branchId?: string;
}

export type WorkflowAuthoringIssueCode =
  | 'missing-node'
  | 'self-connection'
  | 'end-source'
  | 'start-target'
  | 'invalid-source-handle'
  | 'occupied-source-handle'
  | 'duplicate-connection'
  | 'required-field'
  | 'unconnected-branch'
  | 'too-few-candidates'
  | 'splitter-scope';

export interface WorkflowAuthoringIssue {
  code: WorkflowAuthoringIssueCode;
  message: string;
  nodeId?: string;
  edgeId?: string;
  field?: string;
  splitScopeCode?: WorkflowSplitScopeIssueCode;
}

export interface WorkflowIdFactory {
  next(prefix: 'node' | 'edge' | 'branch'): string;
}

export type WorkflowAuthoringCommand =
  | {
      type: 'create-node';
      nodeType: Exclude<WorkflowNodeKind, 'start' | 'end'>;
      position: WorkflowNodePosition;
      nodeId?: string;
      data?: Partial<WorkflowNodeData>;
    }
  | {
      type: 'configure-node';
      nodeId: string;
      patch: Partial<WorkflowNodeData>;
    }
  | {
      type: 'move-nodes';
      positions: Record<string, WorkflowNodePosition>;
    }
  | {
      type: 'connect';
      source: string;
      sourceHandle: string;
      target: string;
      edgeId?: string;
    }
  | {
      type: 'create-connected-node';
      source: string;
      sourceHandle: string;
      nodeType: Exclude<WorkflowNodeKind, 'start' | 'end'>;
      position: WorkflowNodePosition;
      nodeId?: string;
      edgeId?: string;
      data?: Partial<WorkflowNodeData>;
    }
  | {
      type: 'reconnect';
      edgeId: string;
      source?: string;
      sourceHandle?: string;
      target?: string;
    }
  | {
      type: 'split-edge';
      edgeId: string;
      nodeType: Exclude<WorkflowNodeKind, 'start' | 'end'>;
      position: WorkflowNodePosition;
      nodeId?: string;
    }
  | {
      type: 'split-edge-with-node';
      edgeId: string;
      nodeId: string;
      position: WorkflowNodePosition;
      firstEdgeId?: string;
      secondEdgeId?: string;
    }
  | { type: 'delete-edge'; edgeId: string }
  | { type: 'delete-nodes'; nodeIds: string[] }
  | {
      type: 'duplicate-node';
      nodeId: string;
      duplicateId?: string;
      position?: WorkflowNodePosition;
      data?: Partial<WorkflowNodeData>;
    };

interface EntityPatch<T> {
  id: string;
  before?: T;
  after?: T;
  beforeIndex?: number;
  afterIndex?: number;
}

export interface WorkflowDraftPatch {
  metadata: {
    before: WorkflowGraphMetadata;
    after: WorkflowGraphMetadata;
  } | null;
  nodes: EntityPatch<WorkflowNode>[];
  edges: EntityPatch<WorkflowEdge>[];
}

type WorkflowGraphMetadata = Omit<WorkflowGraph, 'nodes' | 'edges'>;

export interface WorkflowCommandHistoryEntry {
  label: WorkflowAuthoringCommand['type'];
  forward: WorkflowDraftPatch;
  inverse: WorkflowDraftPatch;
}

export interface WorkflowAuthoringState {
  graph: WorkflowGraph;
  persistedGraph: WorkflowGraph;
  clientRevision: number;
  serverRevision: number;
  dirty: boolean;
  undoStack: WorkflowCommandHistoryEntry[];
  redoStack: WorkflowCommandHistoryEntry[];
  selectedNodeIds: string[];
  selectedEdgeId: string | null;
}

export interface WorkflowCommandResult {
  state: WorkflowAuthoringState;
  issue?: WorkflowAuthoringIssue;
  createdNodeId?: string;
  createdEdgeId?: string;
}

export interface WorkflowSaveSnapshot {
  graph: WorkflowGraph;
  clientRevision: number;
  expectedRevision: number;
}

const defaultIdFactory: WorkflowIdFactory = {
  next(prefix) {
    return `${prefix}-${crypto.randomUUID().slice(0, 8)}`;
  },
};

export function getWorkflowNodeSourceHandles(
  node: WorkflowNode
): WorkflowSemanticHandle[] {
  switch (node.type) {
    case 'end':
      return [];
    case 'condition':
      return (node.data.branches ?? []).map((branch, index) => {
        const branchId =
          branch.id ??
          `branch-${node.id}-${String(index + 1).padStart(2, '0')}`;
        return {
          id: `branch:${branchId}`,
          kind: 'condition_branch' as const,
          label: branch.condition?.trim() || `Branch ${index + 1}`,
          branchId,
        };
      });
    case 'human_gate':
      return node.data.required_action === 'approve'
        ? [
            {
              id: WORKFLOW_SEMANTIC_HANDLE_IDS.approve,
              kind: 'approve',
              label: 'Approve',
            },
          ]
        : [
            {
              id: WORKFLOW_SEMANTIC_HANDLE_IDS.approve,
              kind: 'approve',
              label: 'Approve',
            },
            {
              id: WORKFLOW_SEMANTIC_HANDLE_IDS.reject,
              kind: 'reject',
              label: 'Reject',
            },
          ];
    case 'arena':
      return [
        {
          id: WORKFLOW_SEMANTIC_HANDLE_IDS.winner,
          kind: 'winner',
          label: 'Winner',
        },
      ];
    default:
      return [
        {
          id: WORKFLOW_SEMANTIC_HANDLE_IDS.default,
          kind: 'default',
          label: 'Next',
        },
      ];
  }
}

export function getWorkflowEdgeKindForHandle(
  source: WorkflowNode,
  sourceHandle: string
): WorkflowEdgeKind | null {
  const handle = getWorkflowNodeSourceHandles(source).find(
    (candidate) => candidate.id === sourceHandle
  );
  if (!handle) return null;
  switch (handle.kind) {
    case 'condition_branch':
      return 'condition_branch';
    case 'approve':
      return 'approval';
    case 'reject':
      return 'rejection';
    case 'winner':
      return 'arena_winner';
    default:
      return 'default';
  }
}

export function validateWorkflowConnection(
  graph: WorkflowGraph,
  connection: {
    source: string;
    sourceHandle: string;
    target: string;
    ignoredEdgeId?: string;
  }
): WorkflowAuthoringIssue | null {
  const source = graph.nodes.find((node) => node.id === connection.source);
  const target = graph.nodes.find((node) => node.id === connection.target);
  if (!source || !target) {
    return { code: 'missing-node', message: 'Connection endpoint is missing.' };
  }
  if (source.id === target.id) {
    return {
      code: 'self-connection',
      message: 'A Node cannot connect to itself.',
      nodeId: source.id,
    };
  }
  if (source.type === 'end') {
    return {
      code: 'end-source',
      message: 'End cannot start a connection.',
      nodeId: source.id,
    };
  }
  if (target.type === 'start') {
    return {
      code: 'start-target',
      message: 'Start cannot receive a connection.',
      nodeId: target.id,
    };
  }
  if (!getWorkflowEdgeKindForHandle(source, connection.sourceHandle)) {
    return {
      code: 'invalid-source-handle',
      message: 'Choose a valid route from the source Node.',
      nodeId: source.id,
    };
  }
  const occupiedSingleTargetHandle = graph.edges.find(
    (edge) =>
      edge.id !== connection.ignoredEdgeId &&
      edge.source === source.id &&
      edge.source_handle === connection.sourceHandle &&
      connection.sourceHandle.startsWith('branch:')
  );
  if (occupiedSingleTargetHandle) {
    return {
      code: 'occupied-source-handle',
      message: 'This Condition branch is already connected.',
      edgeId: occupiedSingleTargetHandle.id,
      nodeId: source.id,
    };
  }
  const duplicate = graph.edges.find(
    (edge) =>
      edge.id !== connection.ignoredEdgeId &&
      edge.source === source.id &&
      edge.source_handle === connection.sourceHandle &&
      edge.target === target.id
  );
  return duplicate
    ? {
        code: 'duplicate-connection',
        message: 'This route is already connected to the target Node.',
        edgeId: duplicate.id,
      }
    : null;
}

export function validateWorkflowAuthoringGraph(
  graph: WorkflowGraph
): WorkflowAuthoringIssue[] {
  const issues: WorkflowAuthoringIssue[] = [];
  for (const node of graph.nodes) {
    const title = node.data.display_name;
    if (typeof title !== 'string' || !title.trim()) {
      issues.push(
        requiredIssue(node.id, 'display_name', 'Give this Node a title.')
      );
    }
    if (node.type === 'agent' || node.type === 'task_splitter') {
      if (!node.data.prompt_template?.trim()) {
        issues.push(
          requiredIssue(node.id, 'prompt_template', 'Add the Task prompt.')
        );
      }
      if (!coerceWorkflowNodeExecutorConfig(node.data.executor_config)) {
        issues.push(
          requiredIssue(node.id, 'executor_config', 'Choose an Agent.')
        );
      }
    }
    if (node.type === 'condition') {
      const branches = node.data.branches ?? [];
      if (branches.length === 0) {
        issues.push(
          requiredIssue(
            node.id,
            'branches',
            'Add at least one Condition branch.'
          )
        );
      }
      for (const branch of branches) {
        const branchEdges = graph.edges.filter(
          (edge) =>
            edge.source === node.id &&
            edge.source_handle === `branch:${branch.id}`
        );
        if (branchEdges.length > 1) {
          issues.push({
            code: 'occupied-source-handle',
            message: 'This Condition branch is already connected.',
            nodeId: node.id,
            edgeId: branchEdges[1].id,
            field: 'branches',
          });
        }
        if (!branch.condition?.trim()) {
          issues.push(
            requiredIssue(
              node.id,
              'branches',
              'Describe each branch condition.'
            )
          );
        }
        if (!branch.target_node_id) {
          issues.push({
            code: 'unconnected-branch',
            message: 'Connect each Condition branch to a target Node.',
            nodeId: node.id,
            field: 'branches',
          });
        }
      }
    }
    if (node.type === 'human_gate' && !node.data.prompt_to_human?.trim()) {
      issues.push(
        requiredIssue(node.id, 'prompt_to_human', 'Add the approval request.')
      );
    }
    if (node.type === 'transform') {
      const mode = node.data.mode ?? 'template';
      if (mode === 'template' && !node.data.template?.trim()) {
        issues.push(
          requiredIssue(node.id, 'template', 'Add the transform template.')
        );
      }
      if (mode === 'regex_extract' && !node.data.regex?.trim()) {
        issues.push(
          requiredIssue(node.id, 'regex', 'Add the extraction pattern.')
        );
      }
      if (
        mode === 'truncate' &&
        (!Number.isInteger(node.data.max_chars) ||
          (node.data.max_chars ?? 0) <= 0)
      ) {
        issues.push(
          requiredIssue(
            node.id,
            'max_chars',
            'Maximum characters must be greater than zero.'
          )
        );
      }
    }
    if (node.type === 'arena' && (node.data.attempts?.length ?? 0) < 2) {
      issues.push({
        code: 'too-few-candidates',
        message: 'Arena needs at least two candidates.',
        nodeId: node.id,
        field: 'attempts',
      });
    }
  }
  for (const issue of getWorkflowSplitScopes(graph).issues) {
    issues.push({
      code: 'splitter-scope',
      nodeId: issue.nodeId,
      splitScopeCode: issue.code,
      message: issue.code,
    });
  }
  return issues;
}

export function createWorkflowAuthoringState(
  graph: WorkflowGraph,
  serverRevision = 0
): WorkflowAuthoringState {
  const canonical = canonicalizeWorkflowAuthoringGraph(graph);
  return {
    graph: canonical,
    persistedGraph: clone(canonical),
    clientRevision: 0,
    serverRevision,
    dirty: false,
    undoStack: [],
    redoStack: [],
    selectedNodeIds: [],
    selectedEdgeId: null,
  };
}

export function dispatchWorkflowAuthoringCommand(
  state: WorkflowAuthoringState,
  command: WorkflowAuthoringCommand,
  idFactory: WorkflowIdFactory = defaultIdFactory
): WorkflowCommandResult {
  const transformed = transformGraph(state.graph, command, idFactory);
  if ('issue' in transformed) return { state, issue: transformed.issue };
  const nextGraph = canonicalizeWorkflowAuthoringGraph(transformed.graph);
  const forward = diffGraph(state.graph, nextGraph);
  if (
    !forward.metadata &&
    forward.nodes.length === 0 &&
    forward.edges.length === 0
  ) {
    return { state };
  }
  const entry: WorkflowCommandHistoryEntry = {
    label: command.type,
    forward,
    inverse: invertPatch(forward),
  };
  const graph = applyPatch(state.graph, forward);
  return {
    state: withGraph(
      {
        ...state,
        graph,
        clientRevision: state.clientRevision + 1,
        undoStack: [...state.undoStack, entry],
        redoStack: [],
        selectedNodeIds: transformed.selectedNodeIds ?? state.selectedNodeIds,
        selectedEdgeId:
          transformed.selectedEdgeId === undefined
            ? state.selectedEdgeId
            : transformed.selectedEdgeId,
      },
      graph
    ),
    createdNodeId: transformed.createdNodeId,
    createdEdgeId: transformed.createdEdgeId,
  };
}

export function commitWorkflowAuthoringGraph(
  state: WorkflowAuthoringState,
  graph: WorkflowGraph,
  label: WorkflowCommandHistoryEntry['label'] = 'configure-node'
): WorkflowAuthoringState {
  const nextGraph = canonicalizeWorkflowAuthoringGraph(graph);
  const forward = diffGraph(state.graph, nextGraph);
  if (
    !forward.metadata &&
    forward.nodes.length === 0 &&
    forward.edges.length === 0
  ) {
    return state;
  }
  const entry: WorkflowCommandHistoryEntry = {
    label,
    forward,
    inverse: invertPatch(forward),
  };
  const committed = applyPatch(state.graph, forward);
  return withGraph(
    {
      ...state,
      graph: committed,
      clientRevision: state.clientRevision + 1,
      undoStack: [...state.undoStack, entry],
      redoStack: [],
    },
    committed
  );
}

export function undoWorkflowAuthoring(
  state: WorkflowAuthoringState
): WorkflowAuthoringState {
  const entry = state.undoStack.at(-1);
  if (!entry) return state;
  const graph = applyPatch(state.graph, entry.inverse);
  return withGraph(
    {
      ...state,
      graph,
      clientRevision: state.clientRevision + 1,
      undoStack: state.undoStack.slice(0, -1),
      redoStack: [...state.redoStack, entry],
      selectedNodeIds: state.selectedNodeIds.filter((id) =>
        graph.nodes.some((node) => node.id === id)
      ),
      selectedEdgeId: graph.edges.some(
        (edge) => edge.id === state.selectedEdgeId
      )
        ? state.selectedEdgeId
        : null,
    },
    graph
  );
}

export function redoWorkflowAuthoring(
  state: WorkflowAuthoringState
): WorkflowAuthoringState {
  const entry = state.redoStack.at(-1);
  if (!entry) return state;
  const graph = applyPatch(state.graph, entry.forward);
  return withGraph(
    {
      ...state,
      graph,
      clientRevision: state.clientRevision + 1,
      undoStack: [...state.undoStack, entry],
      redoStack: state.redoStack.slice(0, -1),
    },
    graph
  );
}

export function createWorkflowSaveSnapshot(
  state: WorkflowAuthoringState
): WorkflowSaveSnapshot {
  return deepFreeze({
    graph: canonicalizeWorkflowAuthoringGraph(state.graph),
    clientRevision: state.clientRevision,
    expectedRevision: state.serverRevision,
  });
}

export function acknowledgeWorkflowSave(
  state: WorkflowAuthoringState,
  snapshot: WorkflowSaveSnapshot,
  serverRevision: number
): WorkflowAuthoringState {
  if (
    snapshot.expectedRevision !== state.serverRevision ||
    serverRevision <= state.serverRevision
  ) {
    return state;
  }
  const persistedGraph = clone(snapshot.graph);
  return withGraph(
    { ...state, persistedGraph, serverRevision },
    state.graph,
    persistedGraph
  );
}

/** Publication changes share the revision but do not save the graph draft. */
export function acknowledgeWorkflowPublication(
  state: WorkflowAuthoringState,
  expectedRevision: number,
  serverRevision: number
): WorkflowAuthoringState {
  if (
    expectedRevision !== state.serverRevision ||
    serverRevision <= state.serverRevision
  ) {
    return state;
  }
  return { ...state, serverRevision };
}

export function acknowledgeLocalWorkflowSave(
  state: WorkflowAuthoringState,
  snapshot: WorkflowSaveSnapshot
): WorkflowAuthoringState {
  if (snapshot.expectedRevision !== state.serverRevision) return state;
  const persistedGraph = clone(snapshot.graph);
  return withGraph({ ...state, persistedGraph }, state.graph, persistedGraph);
}

export function canonicalizeWorkflowAuthoringGraph(
  graph: WorkflowGraph
): WorkflowGraph {
  const nodes = graph.nodes.map((node, nodeIndex) => {
    const data = clone(node.data);
    delete data.selected_skills;
    if (node.type === 'condition') {
      data.branches = (data.branches ?? []).map((branch, branchIndex) => ({
        ...branch,
        id:
          branch.id ??
          `branch-${node.id}-${String(branchIndex + 1).padStart(2, '0')}`,
      }));
    }
    return {
      ...node,
      data,
      position: node.position ?? { x: 120 + nodeIndex * 300, y: 160 },
    };
  });
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const edges = graph.edges.map((edge) => {
    const source = nodeById.get(edge.source);
    if (!source) return clone(edge);
    const sourceHandle = resolveSemanticSourceHandle(source, edge);
    return {
      ...clone(edge),
      source_handle: sourceHandle,
      target_handle:
        !edge.target_handle ||
        LEGACY_POSITIONAL_HANDLE_IDS.has(edge.target_handle)
          ? WORKFLOW_SEMANTIC_HANDLE_IDS.input
          : edge.target_handle,
      type: getWorkflowEdgeKindForHandle(source, sourceHandle) ?? edge.type,
    };
  });
  const connectedBranchById = new Map<string, string>();
  for (const edge of edges) {
    if (edge.source_handle?.startsWith('branch:')) {
      connectedBranchById.set(
        `${edge.source}:${edge.source_handle.slice('branch:'.length)}`,
        edge.target
      );
    }
  }
  return {
    ...clone(graph),
    version: WORKFLOW_GRAPH_VERSION,
    nodes: nodes.map((node) =>
      node.type !== 'condition'
        ? node
        : {
            ...node,
            data: {
              ...node.data,
              branches: (node.data.branches ?? []).map((branch) => ({
                ...branch,
                target_node_id: branch.id
                  ? connectedBranchById.get(`${node.id}:${branch.id}`)
                  : undefined,
              })),
            },
          }
    ),
    edges,
  };
}

type GraphTransform = {
  graph: WorkflowGraph;
  selectedNodeIds?: string[];
  selectedEdgeId?: string | null;
  createdNodeId?: string;
  createdEdgeId?: string;
};

function transformGraph(
  graph: WorkflowGraph,
  command: WorkflowAuthoringCommand,
  idFactory: WorkflowIdFactory
): GraphTransform | { issue: WorkflowAuthoringIssue } {
  switch (command.type) {
    case 'create-node': {
      const nodeId = command.nodeId ?? idFactory.next('node');
      const node = createWorkflowNode(command.nodeType, {
        id: nodeId,
        data: command.data,
        position: command.position,
      });
      return {
        graph: { ...graph, nodes: [...graph.nodes, node] },
        selectedNodeIds: [nodeId],
        selectedEdgeId: null,
        createdNodeId: nodeId,
      };
    }
    case 'configure-node': {
      const configuredNode = graph.nodes.find(
        (node) => node.id === command.nodeId
      );
      if (!configuredNode) return missingNodeIssue();

      let edges = graph.edges;
      if (
        configuredNode.type === 'condition' &&
        command.patch.branches !== undefined
      ) {
        const retainedHandles = new Set(
          (command.patch.branches ?? [])
            .map((branch) => branch.id)
            .filter((branchId): branchId is string => Boolean(branchId))
            .map((branchId) => `branch:${branchId}`)
        );
        edges = edges.filter(
          (edge) =>
            edge.source !== configuredNode.id ||
            !edge.source_handle?.startsWith('branch:') ||
            retainedHandles.has(edge.source_handle)
        );
      }
      if (
        configuredNode.type === 'human_gate' &&
        command.patch.required_action === 'approve'
      ) {
        edges = edges.filter(
          (edge) =>
            edge.source !== configuredNode.id ||
            (edge.source_handle !== WORKFLOW_SEMANTIC_HANDLE_IDS.reject &&
              edge.type !== 'rejection')
        );
      }

      return {
        graph: {
          ...graph,
          nodes: graph.nodes.map((node) =>
            node.id === command.nodeId
              ? { ...node, data: { ...node.data, ...clone(command.patch) } }
              : node
          ),
          edges,
        },
      };
    }
    case 'move-nodes':
      return {
        graph: {
          ...graph,
          nodes: graph.nodes.map((node) =>
            command.positions[node.id]
              ? { ...node, position: command.positions[node.id] }
              : node
          ),
        },
      };
    case 'connect':
      return connectGraph(graph, command, idFactory);
    case 'create-connected-node': {
      const nodeId = command.nodeId ?? idFactory.next('node');
      const edgeId = command.edgeId ?? idFactory.next('edge');
      const node = createWorkflowNode(command.nodeType, {
        id: nodeId,
        data: command.data,
        position: command.position,
      });
      const graphWithNode = { ...graph, nodes: [...graph.nodes, node] };
      const connected = connectGraph(
        graphWithNode,
        {
          type: 'connect',
          source: command.source,
          sourceHandle: command.sourceHandle,
          target: nodeId,
          edgeId,
        },
        idFactory
      );
      if ('task' in connected) return connected;
      return {
        ...connected,
        selectedNodeIds: [nodeId],
        selectedEdgeId: null,
        createdNodeId: nodeId,
        createdEdgeId: edgeId,
      };
    }
    case 'reconnect': {
      const edge = graph.edges.find(
        (candidate) => candidate.id === command.edgeId
      );
      if (!edge) return missingNodeIssue();
      const connection = {
        source: command.source ?? edge.source,
        sourceHandle: command.sourceHandle ?? edge.source_handle ?? '',
        target: command.target ?? edge.target,
        ignoredEdgeId: edge.id,
      };
      const issue = validateWorkflowConnection(graph, connection);
      if (issue) return { issue };
      return {
        graph: {
          ...graph,
          edges: graph.edges.map((candidate) =>
            candidate.id === edge.id
              ? createWorkflowEdge({
                  ...candidate,
                  source: connection.source,
                  source_handle: connection.sourceHandle,
                  target: connection.target,
                  target_handle: WORKFLOW_SEMANTIC_HANDLE_IDS.input,
                })
              : candidate
          ),
        },
        selectedEdgeId: edge.id,
        selectedNodeIds: [],
      };
    }
    case 'split-edge': {
      const edge = graph.edges.find(
        (candidate) => candidate.id === command.edgeId
      );
      if (!edge) return missingNodeIssue();
      const nodeId = command.nodeId ?? idFactory.next('node');
      const node = createWorkflowNode(command.nodeType, {
        id: nodeId,
        position: command.position,
      });
      const first = createWorkflowEdge({
        id: idFactory.next('edge'),
        source: edge.source,
        source_handle: edge.source_handle,
        target: nodeId,
        target_handle: WORKFLOW_SEMANTIC_HANDLE_IDS.input,
        type: edge.type,
        data: edge.data ? clone(edge.data) : undefined,
      });
      const second = createWorkflowEdge({
        id: idFactory.next('edge'),
        source: nodeId,
        source_handle: WORKFLOW_SEMANTIC_HANDLE_IDS.default,
        target: edge.target,
        target_handle: edge.target_handle ?? WORKFLOW_SEMANTIC_HANDLE_IDS.input,
      });
      return {
        graph: {
          ...graph,
          nodes: [...graph.nodes, node],
          edges: [
            ...graph.edges.filter((item) => item.id !== edge.id),
            first,
            second,
          ],
        },
        selectedNodeIds: [nodeId],
        selectedEdgeId: null,
        createdNodeId: nodeId,
      };
    }
    case 'split-edge-with-node': {
      const edge = graph.edges.find(
        (candidate) => candidate.id === command.edgeId
      );
      const node = graph.nodes.find(
        (candidate) => candidate.id === command.nodeId
      );
      if (
        !edge ||
        !node ||
        node.type === 'start' ||
        node.type === 'end' ||
        edge.source === node.id ||
        edge.target === node.id
      ) {
        return missingNodeIssue();
      }
      const firstConnection = {
        source: edge.source,
        sourceHandle: edge.source_handle ?? '',
        target: node.id,
        ignoredEdgeId: edge.id,
      };
      const secondConnection = {
        source: node.id,
        sourceHandle: WORKFLOW_SEMANTIC_HANDLE_IDS.default,
        target: edge.target,
        ignoredEdgeId: edge.id,
      };
      const issue =
        validateWorkflowConnection(graph, firstConnection) ??
        validateWorkflowConnection(graph, secondConnection);
      if (issue) return { issue };
      const first = createWorkflowEdge({
        id: command.firstEdgeId ?? idFactory.next('edge'),
        source: edge.source,
        source_handle: edge.source_handle,
        target: node.id,
        target_handle: WORKFLOW_SEMANTIC_HANDLE_IDS.input,
        type: edge.type,
        data: edge.data ? clone(edge.data) : undefined,
      });
      const second = createWorkflowEdge({
        id: command.secondEdgeId ?? idFactory.next('edge'),
        source: node.id,
        source_handle: WORKFLOW_SEMANTIC_HANDLE_IDS.default,
        target: edge.target,
        target_handle: edge.target_handle ?? WORKFLOW_SEMANTIC_HANDLE_IDS.input,
      });
      return {
        graph: {
          ...graph,
          nodes: graph.nodes.map((candidate) =>
            candidate.id === node.id
              ? { ...candidate, position: command.position }
              : candidate
          ),
          edges: [
            ...graph.edges.filter((candidate) => candidate.id !== edge.id),
            first,
            second,
          ],
        },
        selectedNodeIds: [node.id],
        selectedEdgeId: null,
      };
    }
    case 'delete-edge':
      return {
        graph: {
          ...graph,
          edges: graph.edges.filter((edge) => edge.id !== command.edgeId),
        },
        selectedEdgeId: null,
      };
    case 'delete-nodes': {
      const requested = new Set(command.nodeIds);
      const deletable = new Set(
        graph.nodes
          .filter(
            (node) =>
              requested.has(node.id) &&
              node.type !== 'start' &&
              node.type !== 'end'
          )
          .map((node) => node.id)
      );
      return {
        graph: {
          ...graph,
          nodes: graph.nodes.filter((node) => !deletable.has(node.id)),
          edges: graph.edges.filter(
            (edge) => !deletable.has(edge.source) && !deletable.has(edge.target)
          ),
        },
        selectedNodeIds: [],
        selectedEdgeId: null,
      };
    }
    case 'duplicate-node': {
      const original = graph.nodes.find((node) => node.id === command.nodeId);
      if (!original || original.type === 'start' || original.type === 'end') {
        return missingNodeIssue();
      }
      const nodeId = command.duplicateId ?? idFactory.next('node');
      const data = clone(original.data);
      delete data.session_id;
      if (original.type === 'condition') data.branches = [];
      const node: WorkflowNode = {
        ...original,
        id: nodeId,
        data: { ...data, ...clone(command.data ?? {}) },
        position: command.position ?? {
          x: (original.position?.x ?? 0) + 32,
          y: (original.position?.y ?? 0) + 32,
        },
      };
      return {
        graph: { ...graph, nodes: [...graph.nodes, node] },
        selectedNodeIds: [nodeId],
        selectedEdgeId: null,
        createdNodeId: nodeId,
      };
    }
  }
}

export type WorkflowTransformResult =
  | { ok: true; output: string }
  | { ok: false; error: string };

export function applyWorkflowTransform(
  data: WorkflowNodeData,
  input: string
): WorkflowTransformResult {
  const mode = data.mode ?? 'template';
  if (mode === 'template') {
    if (typeof data.template !== 'string' || !data.template.trim()) {
      return { ok: false, error: 'Template text is required.' };
    }
    return {
      ok: true,
      output: data.template
        .replaceAll('{{input}}', input)
        .replaceAll('{{upstream}}', input),
    };
  }
  if (mode === 'regex_extract') {
    if (typeof data.regex !== 'string' || !data.regex.trim()) {
      return { ok: false, error: 'Regular expression is required.' };
    }
    try {
      const match = new RegExp(data.regex).exec(input);
      if (!match) return { ok: false, error: 'No match found.' };
      return { ok: true, output: match[1] ?? match[0] };
    } catch (error) {
      return {
        ok: false,
        error:
          error instanceof Error
            ? error.message
            : 'Invalid regular expression.',
      };
    }
  }
  if (!Number.isInteger(data.max_chars) || (data.max_chars ?? 0) <= 0) {
    return {
      ok: false,
      error: 'Maximum characters must be greater than zero.',
    };
  }
  return {
    ok: true,
    output: Array.from(input).slice(0, data.max_chars).join(''),
  };
}

function connectGraph(
  graph: WorkflowGraph,
  command: Extract<WorkflowAuthoringCommand, { type: 'connect' }>,
  idFactory: WorkflowIdFactory
): GraphTransform | { issue: WorkflowAuthoringIssue } {
  const issue = validateWorkflowConnection(graph, command);
  if (issue) return { issue };
  const source = graph.nodes.find((node) => node.id === command.source)!;
  const edgeId = command.edgeId ?? idFactory.next('edge');
  const edge = createWorkflowEdge({
    id: edgeId,
    source: source.id,
    source_handle: command.sourceHandle,
    target: command.target,
    target_handle: WORKFLOW_SEMANTIC_HANDLE_IDS.input,
    type:
      getWorkflowEdgeKindForHandle(source, command.sourceHandle) ?? 'default',
  });
  return {
    graph: { ...graph, edges: [...graph.edges, edge] },
    selectedNodeIds: [],
    selectedEdgeId: edgeId,
    createdEdgeId: edgeId,
  };
}

function diffGraph(
  before: WorkflowGraph,
  after: WorkflowGraph
): WorkflowDraftPatch {
  const beforeMetadata = getWorkflowGraphMetadata(before);
  const afterMetadata = getWorkflowGraphMetadata(after);
  return {
    metadata:
      JSON.stringify(beforeMetadata) === JSON.stringify(afterMetadata)
        ? null
        : { before: beforeMetadata, after: afterMetadata },
    nodes: diffEntities(before.nodes, after.nodes),
    edges: diffEntities(before.edges, after.edges),
  };
}

function diffEntities<T extends { id: string }>(
  before: T[],
  after: T[]
): EntityPatch<T>[] {
  const beforeMap = new Map(
    before.map((item, index) => [item.id, { item, index }])
  );
  const afterMap = new Map(
    after.map((item, index) => [item.id, { item, index }])
  );
  const ids = new Set([...beforeMap.keys(), ...afterMap.keys()]);
  const changes: EntityPatch<T>[] = [];
  for (const id of ids) {
    const previous = beforeMap.get(id);
    const next = afterMap.get(id);
    if (JSON.stringify(previous?.item) === JSON.stringify(next?.item)) continue;
    changes.push({
      id,
      before: previous ? clone(previous.item) : undefined,
      after: next ? clone(next.item) : undefined,
      beforeIndex: previous?.index,
      afterIndex: next?.index,
    });
  }
  return changes;
}

function invertPatch(patch: WorkflowDraftPatch): WorkflowDraftPatch {
  const invert = <T>(change: EntityPatch<T>): EntityPatch<T> => ({
    id: change.id,
    before: change.after,
    after: change.before,
    beforeIndex: change.afterIndex,
    afterIndex: change.beforeIndex,
  });
  return {
    metadata: patch.metadata
      ? {
          before: clone(patch.metadata.after),
          after: clone(patch.metadata.before),
        }
      : null,
    nodes: patch.nodes.map(invert),
    edges: patch.edges.map(invert),
  };
}

function applyPatch(
  graph: WorkflowGraph,
  patch: WorkflowDraftPatch
): WorkflowGraph {
  return {
    ...(patch.metadata?.after ?? getWorkflowGraphMetadata(graph)),
    nodes: applyEntityPatch(graph.nodes, patch.nodes),
    edges: applyEntityPatch(graph.edges, patch.edges),
  };
}

function getWorkflowGraphMetadata(graph: WorkflowGraph): WorkflowGraphMetadata {
  return clone(
    Object.fromEntries(
      Object.entries(graph).filter(
        ([key]) => key !== 'nodes' && key !== 'edges'
      )
    ) as WorkflowGraphMetadata
  );
}

function applyEntityPatch<T extends { id: string }>(
  current: T[],
  changes: EntityPatch<T>[]
): T[] {
  const changeById = new Map(changes.map((change) => [change.id, change]));
  const result = current
    .filter((item) => !changeById.has(item.id))
    .map((item) => clone(item));
  const insertions = changes
    .filter((change): change is EntityPatch<T> & { after: T } =>
      Boolean(change.after)
    )
    .sort(
      (left, right) =>
        (left.afterIndex ?? result.length) - (right.afterIndex ?? result.length)
    );
  for (const insertion of insertions) {
    result.splice(
      Math.min(insertion.afterIndex ?? result.length, result.length),
      0,
      clone(insertion.after)
    );
  }
  return result;
}

function resolveSemanticSourceHandle(
  source: WorkflowNode,
  edge: WorkflowEdge
): string {
  if (source.type === 'condition') {
    if (edge.source_handle?.startsWith('branch:')) return edge.source_handle;
    const branch = source.data.branches?.find(
      (candidate) => candidate.target_node_id === edge.target
    );
    return `branch:${branch?.id ?? `branch-${edge.id}`}`;
  }
  if (source.type === 'human_gate') {
    return edge.type === 'rejection'
      ? WORKFLOW_SEMANTIC_HANDLE_IDS.reject
      : WORKFLOW_SEMANTIC_HANDLE_IDS.approve;
  }
  if (source.type === 'arena') return WORKFLOW_SEMANTIC_HANDLE_IDS.winner;
  return WORKFLOW_SEMANTIC_HANDLE_IDS.default;
}

function requiredIssue(
  nodeId: string,
  field: string,
  message: string
): WorkflowAuthoringIssue {
  return { code: 'required-field', message, nodeId, field };
}

function missingNodeIssue(): { issue: WorkflowAuthoringIssue } {
  return {
    issue: {
      code: 'missing-node',
      message: 'The selected object no longer exists.',
    },
  };
}

function withGraph(
  state: WorkflowAuthoringState,
  graph: WorkflowGraph,
  persistedGraph = state.persistedGraph
): WorkflowAuthoringState {
  return {
    ...state,
    graph,
    persistedGraph,
    dirty: JSON.stringify(graph) !== JSON.stringify(persistedGraph),
  };
}

function clone<T>(value: T): T {
  return structuredClone(value);
}

function deepFreeze<T>(value: T): T {
  if (value && typeof value === 'object' && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const child of Object.values(value as Record<string, unknown>)) {
      deepFreeze(child);
    }
  }
  return value;
}
