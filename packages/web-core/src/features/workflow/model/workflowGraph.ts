import type {
  Node as ReactFlowNode,
  Edge as ReactFlowEdge,
} from '@xyflow/react';
import { createDefaultNodeData } from './workflowNodeCatalog';
import { getWorkflowEdgeLabel } from './workflowPresentation';

export const WORKFLOW_GRAPH_VERSION = 2;
export const WORKFLOW_PORT_HANDLE_IDS = {
  left: 'port-left',
  top: 'port-top',
  right: 'port-right',
  bottom: 'port-bottom',
} as const;

/**
 * Handles persisted by the current Workflow contract. Positional `port-*`
 * IDs remain recognized only while importing older drafts.
 */
const WORKFLOW_SEMANTIC_HANDLE_IDS = {
  input: 'input',
  default: 'default',
  winner: 'winner',
  approve: 'approve',
  reject: 'reject',
} as const;
export const DEFAULT_SOURCE_HANDLE = WORKFLOW_SEMANTIC_HANDLE_IDS.default;
export const DEFAULT_TARGET_HANDLE = WORKFLOW_SEMANTIC_HANDLE_IDS.input;

export type WorkflowNodeKind =
  | 'start'
  | 'end'
  | 'agent'
  | 'task_splitter'
  | 'condition'
  | 'human_gate'
  | 'transform'
  | 'arena';

export const WORKFLOW_NODE_DRAG_DATA_TYPE = 'application/x-vibe-workflow-node';

const WORKFLOW_NODE_KINDS: readonly WorkflowNodeKind[] = [
  'start',
  'end',
  'agent',
  'task_splitter',
  'condition',
  'human_gate',
  'transform',
  'arena',
];

export function isWorkflowNodeKind(value: string): value is WorkflowNodeKind {
  return WORKFLOW_NODE_KINDS.includes(value as WorkflowNodeKind);
}

export type WorkflowEdgeKind =
  | 'default'
  | 'condition_branch'
  | 'approval'
  | 'rejection'
  | 'arena_winner';

export const WORKFLOW_REACT_FLOW_EDGE_TYPE = 'workflow';

const WORKFLOW_EDGE_KINDS: readonly WorkflowEdgeKind[] = [
  'default',
  'condition_branch',
  'approval',
  'rejection',
  'arena_winner',
];

export function isWorkflowEdgeKind(value: unknown): value is WorkflowEdgeKind {
  return (
    typeof value === 'string' &&
    WORKFLOW_EDGE_KINDS.includes(value as WorkflowEdgeKind)
  );
}

export type OutputCaptureMode = 'last_message' | 'full_text' | 'diff_summary';
export type PromoteStrategy = 'manual';
export type ApplyStrategy = 'diff_apply';
export type ConditionRoutingMode = 'single' | 'multi';
export type RequiredAction = 'approve' | 'approve_or_reject';
export type TransformMode = 'template' | 'regex_extract' | 'truncate';

export interface WorkflowConditionBranch {
  id?: string;
  target_node_id?: string;
  condition?: string;
}

export interface WorkflowArenaAttemptConfig {
  id?: string;
  display_name?: string;
  role_template_id?: string;
  executor_config?: unknown;
  prompt_template?: string;
}

export interface WorkflowNodeData extends Record<string, unknown> {
  display_name?: string;
  session_id?: string;
  role_template_id?: string;
  executor_config?: unknown;
  prompt_template?: string;
  include_workflow_context?: boolean;
  output_capture?: OutputCaptureMode;
  attempts?: WorkflowArenaAttemptConfig[];
  promote_strategy?: PromoteStrategy;
  apply_strategy?: ApplyStrategy;
  routing_mode?: ConditionRoutingMode;
  branches?: WorkflowConditionBranch[];
  prompt_to_human?: string;
  required_action?: RequiredAction;
  mode?: TransformMode;
  template?: string;
  regex?: string;
  max_chars?: number;
}

export interface WorkflowNode {
  id: string;
  type: WorkflowNodeKind;
  data: WorkflowNodeData;
  position?: WorkflowNodePosition;
}

export interface WorkflowEdge {
  id: string;
  source: string;
  source_handle?: string;
  target: string;
  target_handle?: string;
  type: WorkflowEdgeKind;
  data?: WorkflowEdgeData;
}

export interface WorkflowEdgeData {
  route?: WorkflowEdgeRouteData;
}

export interface WorkflowEdgeRouteData {
  bend?: WorkflowNodePosition;
}

export type WorkflowCanvasObjectKind = 'sticky_note' | 'stage_group';
export type WorkflowCanvasObjectColor = 'amber' | 'blue' | 'green' | 'neutral';

export interface WorkflowCanvasObjectSize {
  width: number;
  height: number;
}

export interface WorkflowCanvasStickyNote {
  id: string;
  type: 'sticky_note';
  title?: string;
  content: string;
  position: WorkflowNodePosition;
  size: WorkflowCanvasObjectSize;
  color?: WorkflowCanvasObjectColor;
}

export interface WorkflowCanvasStageGroup {
  id: string;
  type: 'stage_group';
  title: string;
  description?: string;
  position: WorkflowNodePosition;
  size: WorkflowCanvasObjectSize;
  color?: WorkflowCanvasObjectColor;
}

export interface WorkflowCanvasData {
  notes?: WorkflowCanvasStickyNote[];
  groups?: WorkflowCanvasStageGroup[];
}

export interface WorkflowGraph {
  version: number;
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  router_executor_config?: unknown;
  canvas?: WorkflowCanvasData;
}

export interface WorkflowConditionBranchTarget {
  nodeId: string;
  label: string;
  edgeIds: string[];
}

export interface WorkflowCanvasObjectNodeData extends Record<string, unknown> {
  title?: string;
  content?: string;
  description?: string;
  color?: WorkflowCanvasObjectColor;
  size?: WorkflowCanvasObjectSize;
}

export type WorkflowCanvasReactFlowNodeKind =
  | WorkflowNodeKind
  | WorkflowCanvasObjectKind;
export type WorkflowCanvasReactFlowNodeData =
  | WorkflowNodeData
  | WorkflowCanvasObjectNodeData;

type LegacyWorkflowEdge = Omit<
  WorkflowEdge,
  'source_handle' | 'target_handle'
> &
  Partial<Pick<WorkflowEdge, 'source_handle' | 'target_handle'>>;

type LegacyWorkflowGraph = Omit<WorkflowGraph, 'edges'> & {
  edges: LegacyWorkflowEdge[];
};

export interface ReactFlowWorkflowEdgeData extends Record<string, unknown> {
  workflowType: WorkflowEdgeKind;
  route?: WorkflowEdgeRouteData;
}

export interface WorkflowNodePosition {
  x: number;
  y: number;
}

const DEFAULT_WORKFLOW_LAYOUT = {
  start: { x: 120, y: 190 },
  agent: { x: 420, y: 160 },
  end: { x: 780, y: 190 },
} as const satisfies Record<string, WorkflowNodePosition>;

export interface WorkflowGraphDefaultLabels {
  startLabel: string;
  endLabel: string;
  familiarizeLabel: string;
  agentPrompt: string;
  stageTitle: string;
  stageDescription: string;
}

const DEFAULT_WORKFLOW_GRAPH_LABELS: WorkflowGraphDefaultLabels = {
  startLabel: 'Start',
  endLabel: 'End',
  familiarizeLabel: 'Understand project',
  agentPrompt:
    'Review the current project structure, key modules, and task context. Summarize your understanding, risks, and next implementation plan.',
  stageTitle: 'Stage 1: Understand project',
  stageDescription:
    'Start by understanding the project, then add implementation, review, or test nodes as needed.',
};

const DEFAULT_NOTE_SIZE: WorkflowCanvasObjectSize = {
  width: 280,
  height: 150,
};

const DEFAULT_GROUP_SIZE: WorkflowCanvasObjectSize = {
  width: 880,
  height: 240,
};

const WORKFLOW_TIDY_ORIGIN_X = 120;
const WORKFLOW_TIDY_CENTER_Y = 180;
const WORKFLOW_TIDY_COLUMN_GAP = 180;
const WORKFLOW_TIDY_ROW_GAP = 88;
const WORKFLOW_TIDY_MIN_TOP = 70;
const WORKFLOW_TIDY_ORDER_SWEEPS = 4;

const WORKFLOW_TIDY_NODE_SIZES = {
  start: { width: 140, height: 64 },
  end: { width: 140, height: 64 },
  agent: { width: 280, height: 150 },
  task_splitter: { width: 300, height: 166 },
  condition: { width: 260, height: 132 },
  human_gate: { width: 260, height: 132 },
  transform: { width: 260, height: 132 },
  arena: { width: 280, height: 170 },
} as const satisfies Record<WorkflowNodeKind, WorkflowCanvasObjectSize>;

// These dimensions are only a first-render hint for React Flow. The custom
// node measures its real content immediately afterwards; keeping the hint
// close to the rendered card prevents a large blank drag area on first paint.
const WORKFLOW_NODE_INITIAL_SIZES = {
  start: { width: 140, height: 64 },
  end: { width: 140, height: 64 },
  agent: { width: 232, height: 64 },
  task_splitter: { width: 232, height: 72 },
  condition: { width: 232, height: 64 },
  human_gate: { width: 232, height: 64 },
  transform: { width: 232, height: 64 },
  arena: { width: 232, height: 96 },
} as const satisfies Record<WorkflowNodeKind, WorkflowCanvasObjectSize>;

const WORKFLOW_TIDY_NODE_KIND_ORDER = {
  start: 0,
  condition: 1,
  agent: 2,
  task_splitter: 3,
  human_gate: 4,
  transform: 5,
  arena: 6,
  end: 7,
} as const satisfies Record<WorkflowNodeKind, number>;

export function normalizeWorkflowPortHandle(
  handle: string | null | undefined,
  fallback: string
): string {
  switch (handle) {
    case 'input-left':
    case 'output-left':
      return WORKFLOW_PORT_HANDLE_IDS.left;
    case 'input-top':
    case 'output-top':
      return WORKFLOW_PORT_HANDLE_IDS.top;
    case 'input-right':
    case 'output-right':
      return WORKFLOW_PORT_HANDLE_IDS.right;
    case 'input-bottom':
    case 'output-bottom':
      return WORKFLOW_PORT_HANDLE_IDS.bottom;
    default:
      return handle ?? fallback;
  }
}

function resolveWorkflowSemanticTargetHandle(
  handle: string | null | undefined
): string {
  if (!handle) return DEFAULT_TARGET_HANDLE;
  if (
    handle === WORKFLOW_PORT_HANDLE_IDS.left ||
    handle === WORKFLOW_PORT_HANDLE_IDS.top ||
    handle === WORKFLOW_PORT_HANDLE_IDS.right ||
    handle === WORKFLOW_PORT_HANDLE_IDS.bottom ||
    handle.startsWith('input-')
  ) {
    return WORKFLOW_SEMANTIC_HANDLE_IDS.input;
  }
  return handle;
}

function resolveWorkflowSemanticSourceHandle(
  source: WorkflowNode | undefined,
  edge: Pick<WorkflowEdge, 'source_handle' | 'target' | 'type' | 'id'>
): string {
  const rawHandle = edge.source_handle;
  if (source?.type === 'condition') {
    if (rawHandle?.startsWith('branch:')) return rawHandle;
    const branch = source.data.branches?.find(
      (candidate) => candidate.target_node_id === edge.target
    );
    return `branch:${branch?.id ?? `branch-${edge.id}`}`;
  }
  if (source?.type === 'human_gate') {
    if (rawHandle === WORKFLOW_SEMANTIC_HANDLE_IDS.reject) return rawHandle;
    return edge.type === 'rejection'
      ? WORKFLOW_SEMANTIC_HANDLE_IDS.reject
      : WORKFLOW_SEMANTIC_HANDLE_IDS.approve;
  }
  if (source?.type === 'arena') return WORKFLOW_SEMANTIC_HANDLE_IDS.winner;
  if (rawHandle && !isLegacyWorkflowPortHandle(rawHandle)) return rawHandle;
  return DEFAULT_SOURCE_HANDLE;
}

function isLegacyWorkflowPortHandle(handle: string): boolean {
  return (
    handle === WORKFLOW_PORT_HANDLE_IDS.left ||
    handle === WORKFLOW_PORT_HANDLE_IDS.top ||
    handle === WORKFLOW_PORT_HANDLE_IDS.right ||
    handle === WORKFLOW_PORT_HANDLE_IDS.bottom ||
    /^(?:input|output)-(?:left|top|right|bottom)$/.test(handle)
  );
}

function normalizeWorkflowSemanticHandles(graph: WorkflowGraph): WorkflowGraph {
  const nodeById = new Map(graph.nodes.map((node) => [node.id, node]));
  const edges = graph.edges.map((edge) => ({
    ...edge,
    source_handle: resolveWorkflowSemanticSourceHandle(
      nodeById.get(edge.source),
      edge
    ),
    target_handle: resolveWorkflowSemanticTargetHandle(edge.target_handle),
  }));
  return { ...graph, edges };
}

function getNodeLabel(
  node: WorkflowNode | undefined,
  fallback: string
): string {
  const displayName = node?.data.display_name;
  return typeof displayName === 'string' && displayName.trim()
    ? displayName
    : fallback;
}

function getConditionOutgoingEdges(
  graph: WorkflowGraph,
  conditionNodeId: string
): WorkflowEdge[] {
  return graph.edges.filter((edge) => edge.source === conditionNodeId);
}

function getBranchId(conditionNodeId: string, targetNodeId: string): string {
  return `branch-${conditionNodeId}-${targetNodeId}`;
}

function findConditionBranchByTarget(
  graph: WorkflowGraph | undefined,
  conditionNodeId: string,
  targetNodeId: string
): WorkflowConditionBranch | undefined {
  return graph?.nodes
    .find((node) => node.id === conditionNodeId && node.type === 'condition')
    ?.data.branches?.find((branch) => branch.target_node_id === targetNodeId);
}

export function getConditionBranchTargets(
  graph: WorkflowGraph,
  conditionNodeId: string
): WorkflowConditionBranchTarget[] {
  const targets = new Map<string, WorkflowConditionBranchTarget>();
  for (const edge of getConditionOutgoingEdges(graph, conditionNodeId)) {
    const existing = targets.get(edge.target);
    if (existing) {
      existing.edgeIds.push(edge.id);
      continue;
    }

    const targetNode = graph.nodes.find((node) => node.id === edge.target);
    targets.set(edge.target, {
      nodeId: edge.target,
      label: getNodeLabel(targetNode, edge.target),
      edgeIds: [edge.id],
    });
  }

  return Array.from(targets.values());
}

export function syncConditionBranches(
  graph: WorkflowGraph,
  previousGraph?: WorkflowGraph
): WorkflowGraph {
  let changed = false;
  const nodes = graph.nodes.map((node) => {
    if (node.type !== 'condition') return node;

    const outgoingTargets = getConditionBranchTargets(graph, node.id);
    const currentByTarget = new Map(
      (node.data.branches ?? [])
        .filter((branch) => branch.target_node_id)
        .map((branch) => [branch.target_node_id as string, branch])
    );
    const previousTargetByEdgeId = new Map(
      previousGraph?.edges
        .filter((edge) => edge.source === node.id)
        .map((edge) => [edge.id, edge.target]) ?? []
    );

    const nextBranches = outgoingTargets.map((target) => {
      const current = currentByTarget.get(target.nodeId);
      if (current) {
        return {
          id: current.id ?? getBranchId(node.id, target.nodeId),
          target_node_id: target.nodeId,
          condition: current.condition ?? '',
        };
      }

      const previousBranch = target.edgeIds
        .map((edgeId) => previousTargetByEdgeId.get(edgeId))
        .filter((targetId): targetId is string => Boolean(targetId))
        .map((targetId) =>
          findConditionBranchByTarget(previousGraph, node.id, targetId)
        )
        .find((branch): branch is WorkflowConditionBranch => Boolean(branch));

      return {
        id: previousBranch?.id ?? getBranchId(node.id, target.nodeId),
        target_node_id: target.nodeId,
        condition: previousBranch?.condition ?? '',
      };
    });

    const conditionData = { ...node.data };
    delete conditionData.conditions;
    delete conditionData.joiner;
    const nextData = {
      ...conditionData,
      routing_mode: node.data.routing_mode ?? 'single',
      branches: nextBranches,
    };
    if (JSON.stringify(nextData) !== JSON.stringify(node.data)) {
      changed = true;
      return { ...node, data: nextData };
    }
    return node;
  });

  return changed ? { ...graph, nodes } : graph;
}

export function normalizeConditionEdgeTypes(
  graph: WorkflowGraph
): WorkflowGraph {
  const conditionNodeIds = new Set(
    graph.nodes
      .filter((node) => node.type === 'condition')
      .map((node) => node.id)
  );
  let changed = false;
  const edges = graph.edges.map((edge) => {
    if (conditionNodeIds.has(edge.source) && edge.type !== 'condition_branch') {
      changed = true;
      return { ...edge, type: 'condition_branch' as const };
    }
    if (
      !conditionNodeIds.has(edge.source) &&
      edge.type === 'condition_branch'
    ) {
      changed = true;
      return { ...edge, type: 'default' as const };
    }
    return edge;
  });

  return changed ? { ...graph, edges } : graph;
}

export function createDefaultWorkflowGraph(
  labels: Partial<WorkflowGraphDefaultLabels> = {}
): WorkflowGraph {
  const copy = { ...DEFAULT_WORKFLOW_GRAPH_LABELS, ...labels };

  return {
    version: WORKFLOW_GRAPH_VERSION,
    nodes: [
      {
        id: 'start',
        type: 'start',
        data: { display_name: copy.startLabel },
        position: DEFAULT_WORKFLOW_LAYOUT.start,
      },
      {
        id: 'familiarize',
        type: 'agent',
        data: {
          display_name: copy.familiarizeLabel,
          role_template_id: 'custom',
          prompt_template: copy.agentPrompt,
        },
        position: DEFAULT_WORKFLOW_LAYOUT.agent,
      },
      {
        id: 'end',
        type: 'end',
        data: { display_name: copy.endLabel },
        position: DEFAULT_WORKFLOW_LAYOUT.end,
      },
    ],
    edges: [
      {
        id: 'start-familiarize',
        source: 'start',
        source_handle: DEFAULT_SOURCE_HANDLE,
        target: 'familiarize',
        target_handle: DEFAULT_TARGET_HANDLE,
        type: 'default',
      },
      {
        id: 'familiarize-end',
        source: 'familiarize',
        source_handle: DEFAULT_SOURCE_HANDLE,
        target: 'end',
        target_handle: DEFAULT_TARGET_HANDLE,
        type: 'default',
      },
    ],
    canvas: {
      groups: [
        {
          id: 'stage-understand',
          type: 'stage_group',
          title: copy.stageTitle,
          description: copy.stageDescription,
          position: { x: 70, y: 105 },
          size: DEFAULT_GROUP_SIZE,
          color: 'neutral',
        },
      ],
    },
  };
}

export function migrateWorkflowGraph(
  graph: WorkflowGraph | LegacyWorkflowGraph
): WorkflowGraph {
  const canvas = normalizeWorkflowCanvas(graph.canvas);
  const graphWithoutCanvas = { ...graph };
  delete graphWithoutCanvas.canvas;
  const migrated = {
    ...graphWithoutCanvas,
    version: WORKFLOW_GRAPH_VERSION,
    nodes: graph.nodes.map((node, index) => ({
      ...node,
      position: node.position ?? fallbackWorkflowNodePosition(index),
    })),
    edges: graph.edges.map((edge) => ({
      ...edge,
      source_handle: normalizeWorkflowPortHandle(
        edge.source_handle,
        DEFAULT_SOURCE_HANDLE
      ),
      target_handle: normalizeWorkflowPortHandle(
        edge.target_handle,
        DEFAULT_TARGET_HANDLE
      ),
    })),
    ...(canvas ? { canvas } : {}),
  };
  const synchronized = syncConditionBranches(
    normalizeConditionEdgeTypes(migrated)
  );
  return normalizeWorkflowSemanticHandles(synchronized);
}

export function instantiateWorkflowGraphTemplate(
  graph: WorkflowGraph | LegacyWorkflowGraph
): WorkflowGraph {
  const migrated = migrateWorkflowGraph(graph);

  return {
    ...migrated,
    nodes: migrated.nodes.map((node) => {
      const data = { ...node.data };
      delete data.session_id;
      return { ...node, data };
    }),
  };
}

function fallbackWorkflowNodePosition(index: number): WorkflowNodePosition {
  return {
    x: 120 + (index % 4) * 360,
    y: 160 + Math.floor(index / 4) * 190,
  };
}

export function createWorkflowNode(
  kind: WorkflowNodeKind,
  options?: {
    id?: string;
    data?: Partial<WorkflowNodeData>;
    position?: WorkflowNodePosition;
  }
): WorkflowNode {
  return {
    id: options?.id ?? `${kind}-${crypto.randomUUID().slice(0, 8)}`,
    type: kind,
    data: {
      ...createDefaultNodeData(kind),
      ...(options?.data ?? {}),
    },
    ...(options?.position ? { position: options.position } : {}),
  };
}

export function createWorkflowEdge(options: {
  id?: string;
  source: string;
  source_handle?: string;
  target: string;
  target_handle?: string;
  type?: WorkflowEdgeKind;
  data?: WorkflowEdgeData;
}): WorkflowEdge {
  return {
    id: options.id ?? `${options.source}-${options.target}`,
    source: options.source,
    source_handle: options.source_handle ?? DEFAULT_SOURCE_HANDLE,
    target: options.target,
    target_handle: options.target_handle ?? DEFAULT_TARGET_HANDLE,
    type: options.type ?? 'default',
    ...(options.data ? { data: options.data } : {}),
  };
}

export function createWorkflowCanvasStickyNote(options?: {
  id?: string;
  title?: string;
  content?: string;
  position?: WorkflowNodePosition;
  size?: WorkflowCanvasObjectSize;
  color?: WorkflowCanvasObjectColor;
}): WorkflowCanvasStickyNote {
  return {
    id: options?.id ?? `note-${crypto.randomUUID().slice(0, 8)}`,
    type: 'sticky_note',
    title: options?.title ?? 'Note',
    content: options?.content ?? '',
    position: options?.position ?? { x: 160, y: 80 },
    size: options?.size ?? DEFAULT_NOTE_SIZE,
    color: options?.color ?? 'amber',
  };
}

export function createWorkflowCanvasStageGroup(options?: {
  id?: string;
  title?: string;
  description?: string;
  position?: WorkflowNodePosition;
  size?: WorkflowCanvasObjectSize;
  color?: WorkflowCanvasObjectColor;
}): WorkflowCanvasStageGroup {
  return {
    id: options?.id ?? `stage-${crypto.randomUUID().slice(0, 8)}`,
    type: 'stage_group',
    title: options?.title ?? 'Stage',
    description: options?.description ?? '',
    position: options?.position ?? { x: 80, y: 120 },
    size: options?.size ?? DEFAULT_GROUP_SIZE,
    color: options?.color ?? 'neutral',
  };
}

export function toReactFlowNodes(
  graph: WorkflowGraph,
  positions?: Record<string, { x: number; y: number }>
): ReactFlowNode<WorkflowNodeData, WorkflowNodeKind>[] {
  return graph.nodes.map((node) => ({
    id: node.id,
    type: node.type,
    data: node.data,
    position: node.position ?? positions?.[node.id] ?? { x: 0, y: 0 },
    // React Flow hides a node until dimensions are measured. Stable initial
    // dimensions prevent a hidden first render while DOM measurement runs.
    initialWidth: WORKFLOW_NODE_INITIAL_SIZES[node.type].width,
    initialHeight: WORKFLOW_NODE_INITIAL_SIZES[node.type].height,
  }));
}

export function toReactFlowCanvasNodes(
  graph: WorkflowGraph,
  positions?: Record<string, { x: number; y: number }>
): ReactFlowNode<
  WorkflowCanvasReactFlowNodeData,
  WorkflowCanvasReactFlowNodeKind
>[] {
  const groups = (graph.canvas?.groups ?? []).map((group) => ({
    id: group.id,
    type: 'stage_group' as const,
    data: {
      title: group.title,
      description: group.description,
      color: group.color,
      size: group.size,
    },
    position: group.position,
    style: {
      width: group.size.width,
      height: group.size.height,
    },
    zIndex: -1,
  }));

  const notes = (graph.canvas?.notes ?? []).map((note) => ({
    id: note.id,
    type: 'sticky_note' as const,
    data: {
      title: note.title,
      content: note.content,
      color: note.color,
      size: note.size,
    },
    position: note.position,
    style: {
      width: note.size.width,
      height: note.size.height,
    },
    zIndex: 2,
  }));

  return [
    ...groups,
    ...toReactFlowNodes(graph, positions),
    ...notes,
  ] satisfies ReactFlowNode<
    WorkflowCanvasReactFlowNodeData,
    WorkflowCanvasReactFlowNodeKind
  >[];
}

export function toReactFlowEdges(
  graph: WorkflowGraph
): ReactFlowEdge<ReactFlowWorkflowEdgeData>[] {
  const nodeById = new Map(graph.nodes.map((node) => [node.id, node]));
  return graph.edges.map((edge) => ({
    id: edge.id,
    source: edge.source,
    sourceHandle: resolveWorkflowSemanticSourceHandle(
      nodeById.get(edge.source),
      edge
    ),
    target: edge.target,
    targetHandle: resolveWorkflowSemanticTargetHandle(edge.target_handle),
    type: WORKFLOW_REACT_FLOW_EDGE_TYPE,
    data: { ...(edge.data ?? {}), workflowType: edge.type },
    label: getWorkflowEdgeLabel(edge.type),
  }));
}

function getWorkflowTypeFromReactFlowEdge(
  edge: ReactFlowEdge
): WorkflowEdgeKind {
  const data = edge.data as Partial<ReactFlowWorkflowEdgeData> | undefined;
  if (isWorkflowEdgeKind(data?.workflowType)) {
    return data.workflowType;
  }
  if (isWorkflowEdgeKind(edge.type)) {
    return edge.type;
  }
  return 'default';
}

function getWorkflowEdgeDataFromReactFlowEdge(
  edge: ReactFlowEdge
): WorkflowEdgeData | undefined {
  const data = edge.data as Partial<ReactFlowWorkflowEdgeData> | undefined;
  const bend = data?.route?.bend;
  if (!isWorkflowNodePosition(bend)) return undefined;
  return { route: { bend: { x: bend.x, y: bend.y } } };
}

function isWorkflowNodePosition(value: unknown): value is WorkflowNodePosition {
  if (!value || typeof value !== 'object') return false;
  const position = value as Partial<WorkflowNodePosition>;
  return Number.isFinite(position.x) && Number.isFinite(position.y);
}

export function fromReactFlowGraph(
  nodes: ReactFlowNode<WorkflowNodeData, WorkflowNodeKind>[],
  edges: ReactFlowEdge[],
  baseGraph?: WorkflowGraph
): WorkflowGraph {
  const canvas = normalizeWorkflowCanvas(baseGraph?.canvas);
  const baseGraphWithoutCanvas = { ...baseGraph };
  delete baseGraphWithoutCanvas.canvas;
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const nextGraph = {
    ...baseGraphWithoutCanvas,
    version: WORKFLOW_GRAPH_VERSION,
    nodes: nodes.map((node) => ({
      id: node.id,
      type: node.type,
      data: stripWorkflowNodeUiData(node.data),
      position: node.position,
    })),
    edges: edges.map((edge) => {
      const data = getWorkflowEdgeDataFromReactFlowEdge(edge);
      return {
        id: edge.id,
        source: edge.source,
        source_handle: resolveWorkflowSemanticSourceHandle(
          nodeById.get(edge.source),
          {
            id: edge.id,
            source_handle: edge.sourceHandle ?? undefined,
            target: edge.target,
            type: getWorkflowTypeFromReactFlowEdge(edge),
          }
        ),
        target: edge.target,
        target_handle: resolveWorkflowSemanticTargetHandle(edge.targetHandle),
        type: getWorkflowTypeFromReactFlowEdge(edge),
        ...(data ? { data } : {}),
      };
    }),
    ...(canvas ? { canvas } : {}),
  };
  return normalizeWorkflowSemanticHandles(
    syncConditionBranches(normalizeConditionEdgeTypes(nextGraph), baseGraph)
  );
}

export function fromReactFlowCanvasGraph(
  nodes: ReactFlowNode<
    WorkflowCanvasReactFlowNodeData,
    WorkflowCanvasReactFlowNodeKind
  >[],
  edges: ReactFlowEdge[],
  baseGraph?: WorkflowGraph
): WorkflowGraph {
  const workflowNodes: ReactFlowNode<WorkflowNodeData, WorkflowNodeKind>[] = [];
  const notes: WorkflowCanvasStickyNote[] = [];
  const groups: WorkflowCanvasStageGroup[] = [];

  for (const node of nodes) {
    if (isWorkflowNodeKind(String(node.type))) {
      workflowNodes.push(
        node as ReactFlowNode<WorkflowNodeData, WorkflowNodeKind>
      );
      continue;
    }

    if (node.type === 'sticky_note') {
      const data = stripWorkflowCanvasObjectUiData(node.data);
      notes.push({
        id: node.id,
        type: 'sticky_note',
        title: typeof data.title === 'string' ? data.title : undefined,
        content: typeof data.content === 'string' ? data.content : '',
        position: node.position,
        size: getCanvasNodeSize(node, DEFAULT_NOTE_SIZE),
        color: normalizeWorkflowCanvasObjectColor(data.color),
      });
      continue;
    }

    if (node.type === 'stage_group') {
      const data = stripWorkflowCanvasObjectUiData(node.data);
      groups.push({
        id: node.id,
        type: 'stage_group',
        title: typeof data.title === 'string' ? data.title : 'Stage',
        description:
          typeof data.description === 'string' ? data.description : undefined,
        position: node.position,
        size: getCanvasNodeSize(node, DEFAULT_GROUP_SIZE),
        color: normalizeWorkflowCanvasObjectColor(data.color),
      });
    }
  }

  const graph = fromReactFlowGraph(workflowNodes, edges, baseGraph);
  const canvas = normalizeWorkflowCanvas({ notes, groups });
  if (!canvas) {
    const next = { ...graph };
    delete next.canvas;
    return next;
  }
  return { ...graph, canvas };
}

function stripWorkflowNodeUiData(data: WorkflowNodeData): WorkflowNodeData {
  return Object.fromEntries(
    Object.entries(data).filter(([key]) => !key.startsWith('__'))
  ) as WorkflowNodeData;
}

function stripWorkflowCanvasObjectUiData(
  data: WorkflowCanvasReactFlowNodeData
): WorkflowCanvasObjectNodeData {
  return Object.fromEntries(
    Object.entries(data).filter(([key]) => !key.startsWith('__'))
  ) as WorkflowCanvasObjectNodeData;
}

function normalizeWorkflowCanvas(
  canvas: WorkflowCanvasData | undefined
): WorkflowCanvasData | undefined {
  const notes = (canvas?.notes ?? []).map((note) => ({
    ...note,
    type: 'sticky_note' as const,
    content: note.content ?? '',
    position: note.position ?? { x: 160, y: 80 },
    size: normalizeCanvasSize(note.size, DEFAULT_NOTE_SIZE),
    color: normalizeWorkflowCanvasObjectColor(note.color),
  }));
  const groups = (canvas?.groups ?? []).map((group) => ({
    ...group,
    type: 'stage_group' as const,
    title: group.title || 'Stage',
    description: group.description ?? '',
    position: group.position ?? { x: 80, y: 120 },
    size: normalizeCanvasSize(group.size, DEFAULT_GROUP_SIZE),
    color: normalizeWorkflowCanvasObjectColor(group.color),
  }));

  if (notes.length === 0 && groups.length === 0) return undefined;
  return {
    ...(notes.length > 0 ? { notes } : {}),
    ...(groups.length > 0 ? { groups } : {}),
  };
}

function normalizeCanvasSize(
  size: Partial<WorkflowCanvasObjectSize> | undefined,
  fallback: WorkflowCanvasObjectSize
): WorkflowCanvasObjectSize {
  return {
    width:
      typeof size?.width === 'number' && Number.isFinite(size.width)
        ? size.width
        : fallback.width,
    height:
      typeof size?.height === 'number' && Number.isFinite(size.height)
        ? size.height
        : fallback.height,
  };
}

function normalizeWorkflowCanvasObjectColor(
  color: unknown
): WorkflowCanvasObjectColor | undefined {
  return color === 'amber' ||
    color === 'blue' ||
    color === 'green' ||
    color === 'neutral'
    ? color
    : undefined;
}

function getCanvasNodeSize(
  node: ReactFlowNode<
    WorkflowCanvasReactFlowNodeData,
    WorkflowCanvasReactFlowNodeKind
  >,
  fallback: WorkflowCanvasObjectSize
): WorkflowCanvasObjectSize {
  const dataSize =
    'size' in node.data ? (node.data.size as WorkflowCanvasObjectSize) : null;
  return normalizeCanvasSize(
    {
      width:
        node.width ??
        parseCanvasDimension(node.style?.width) ??
        dataSize?.width,
      height:
        node.height ??
        parseCanvasDimension(node.style?.height) ??
        dataSize?.height,
    },
    fallback
  );
}

function parseCanvasDimension(value: unknown): number | undefined {
  if (typeof value === 'number') return value;
  if (typeof value !== 'string') return undefined;
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

export function tidyWorkflowGraph(graph: WorkflowGraph): WorkflowGraph {
  const nodeById = getWorkflowTidyNodeById(graph);
  const adjacency = getWorkflowTidyAdjacency(graph, nodeById);
  const levels = getWorkflowNodeLevels(graph, adjacency);
  const buckets = getWorkflowTidyOrderedBuckets(graph, levels, adjacency);
  const positions = getWorkflowTidyPositions(buckets, adjacency);

  const nodes = graph.nodes.map((node) => ({
    ...node,
    position: positions.get(node.id) ?? node.position,
  }));
  const nextNodeById = getWorkflowTidyNodeById({ ...graph, nodes });
  const edges = graph.edges.map((edge) => {
    const sourceNode = nextNodeById.get(edge.source);
    const targetNode = nextNodeById.get(edge.target);
    if (!sourceNode || !targetNode) return edge;
    return edge;
  });

  return normalizeWorkflowSemanticHandles(
    syncConditionBranches({ ...graph, nodes, edges })
  );
}

interface WorkflowTidyAdjacency {
  incoming: Map<string, WorkflowEdge[]>;
  outgoing: Map<string, WorkflowEdge[]>;
  validEdges: WorkflowEdge[];
}

function getWorkflowTidyNodeById(graph: {
  nodes: WorkflowNode[];
}): Map<string, WorkflowNode> {
  return new Map(graph.nodes.map((node) => [node.id, node]));
}

function getWorkflowTidyAdjacency(
  graph: WorkflowGraph,
  nodeById: ReadonlyMap<string, WorkflowNode>
): WorkflowTidyAdjacency {
  const incoming = new Map<string, WorkflowEdge[]>();
  const outgoing = new Map<string, WorkflowEdge[]>();
  const validEdges: WorkflowEdge[] = [];

  for (const edge of graph.edges) {
    if (
      edge.source === edge.target ||
      !nodeById.has(edge.source) ||
      !nodeById.has(edge.target)
    ) {
      continue;
    }

    validEdges.push(edge);
    const sourceEdges = outgoing.get(edge.source) ?? [];
    sourceEdges.push(edge);
    outgoing.set(edge.source, sourceEdges);

    const targetEdges = incoming.get(edge.target) ?? [];
    targetEdges.push(edge);
    incoming.set(edge.target, targetEdges);
  }

  return { incoming, outgoing, validEdges };
}

function getWorkflowTidyOrderedBuckets(
  graph: WorkflowGraph,
  levels: ReadonlyMap<string, number>,
  adjacency: WorkflowTidyAdjacency
): Map<number, WorkflowNode[]> {
  const buckets = new Map<number, WorkflowNode[]>();

  for (const node of graph.nodes) {
    const level = levels.get(node.id) ?? 0;
    const bucket = buckets.get(level) ?? [];
    bucket.push(node);
    buckets.set(level, bucket);
  }

  for (const [level, bucket] of buckets) {
    buckets.set(level, [...bucket].sort(compareWorkflowTidyOriginalOrder));
  }

  const sortedLevels = getSortedWorkflowTidyLevels(buckets);
  for (let sweep = 0; sweep < WORKFLOW_TIDY_ORDER_SWEEPS; sweep += 1) {
    let orderIndex = getWorkflowTidyOrderIndex(buckets);
    for (const level of sortedLevels) {
      buckets.set(
        level,
        sortWorkflowTidyBucketByNeighbors({
          bucket: buckets.get(level) ?? [],
          edgesByNode: adjacency.incoming,
          getNeighborId: (edge) => edge.source,
          orderIndex,
        })
      );
      orderIndex = getWorkflowTidyOrderIndex(buckets);
    }

    for (const level of [...sortedLevels].reverse()) {
      buckets.set(
        level,
        sortWorkflowTidyBucketByNeighbors({
          bucket: buckets.get(level) ?? [],
          edgesByNode: adjacency.outgoing,
          getNeighborId: (edge) => edge.target,
          orderIndex,
        })
      );
      orderIndex = getWorkflowTidyOrderIndex(buckets);
    }
  }

  return buckets;
}

function sortWorkflowTidyBucketByNeighbors({
  bucket,
  edgesByNode,
  getNeighborId,
  orderIndex,
}: {
  bucket: WorkflowNode[];
  edgesByNode: ReadonlyMap<string, WorkflowEdge[]>;
  getNeighborId: (edge: WorkflowEdge) => string;
  orderIndex: ReadonlyMap<string, number>;
}): WorkflowNode[] {
  const currentIndex = new Map(bucket.map((node, index) => [node.id, index]));
  return [...bucket].sort((a, b) => {
    const aScore = getWorkflowTidyNeighborOrderScore(
      a.id,
      edgesByNode,
      getNeighborId,
      orderIndex
    );
    const bScore = getWorkflowTidyNeighborOrderScore(
      b.id,
      edgesByNode,
      getNeighborId,
      orderIndex
    );
    const aFallback = currentIndex.get(a.id) ?? 0;
    const bFallback = currentIndex.get(b.id) ?? 0;
    const scoreCompare = (aScore ?? aFallback) - (bScore ?? bFallback);
    if (scoreCompare !== 0) return scoreCompare;
    return aFallback - bFallback;
  });
}

function getWorkflowTidyNeighborOrderScore(
  nodeId: string,
  edgesByNode: ReadonlyMap<string, WorkflowEdge[]>,
  getNeighborId: (edge: WorkflowEdge) => string,
  orderIndex: ReadonlyMap<string, number>
): number | null {
  const scores = (edgesByNode.get(nodeId) ?? [])
    .map((edge) => orderIndex.get(getNeighborId(edge)))
    .filter((score): score is number => typeof score === 'number');
  if (scores.length === 0) return null;
  return scores.reduce((total, score) => total + score, 0) / scores.length;
}

function getWorkflowTidyOrderIndex(
  buckets: ReadonlyMap<number, WorkflowNode[]>
): Map<string, number> {
  const index = new Map<string, number>();
  for (const bucket of buckets.values()) {
    bucket.forEach((node, order) => index.set(node.id, order));
  }
  return index;
}

function getWorkflowTidyPositions(
  buckets: ReadonlyMap<number, WorkflowNode[]>,
  adjacency: WorkflowTidyAdjacency
): Map<string, WorkflowNodePosition> {
  const sortedLevels = getSortedWorkflowTidyLevels(buckets);
  const levelX = getWorkflowTidyLevelX(buckets, sortedLevels);
  const centerYByNodeId = new Map<string, number>();

  for (const level of sortedLevels) {
    placeWorkflowTidyBucket({
      bucket: buckets.get(level) ?? [],
      centerYByNodeId,
      getDesiredCenterY: (node) =>
        getWorkflowTidyDesiredCenterY({
          nodeId: node.id,
          edges: adjacency.incoming.get(node.id) ?? [],
          getNeighborId: (edge) => edge.source,
          centerYByNodeId,
        }),
    });
  }

  for (const level of [...sortedLevels].reverse()) {
    placeWorkflowTidyBucket({
      bucket: buckets.get(level) ?? [],
      centerYByNodeId,
      getDesiredCenterY: (node) =>
        getWorkflowTidyDesiredCenterY({
          nodeId: node.id,
          edges: adjacency.outgoing.get(node.id) ?? [],
          getNeighborId: (edge) => edge.target,
          centerYByNodeId,
        }),
    });
  }

  const positions = new Map<string, WorkflowNodePosition>();
  for (const level of sortedLevels) {
    const bucket = buckets.get(level) ?? [];
    const columnWidth = getWorkflowTidyColumnWidth(bucket);
    const x = levelX.get(level) ?? WORKFLOW_TIDY_ORIGIN_X;
    for (const node of bucket) {
      const size = getWorkflowTidyNodeSize(node);
      const centerY = centerYByNodeId.get(node.id) ?? WORKFLOW_TIDY_CENTER_Y;
      positions.set(node.id, {
        x: x + Math.max(0, (columnWidth - size.width) / 2),
        y: centerY - size.height / 2,
      });
    }
  }

  return positions;
}

function placeWorkflowTidyBucket({
  bucket,
  centerYByNodeId,
  getDesiredCenterY,
}: {
  bucket: WorkflowNode[];
  centerYByNodeId: Map<string, number>;
  getDesiredCenterY: (node: WorkflowNode) => number;
}) {
  if (bucket.length === 0) return;

  const relativeCenters = getWorkflowTidyRelativeCenters(bucket);
  const offsets = bucket.map(
    (node, index) => getDesiredCenterY(node) - relativeCenters[index]
  );
  let offset = getWorkflowTidyMedian(offsets);
  const firstSize = getWorkflowTidyNodeSize(bucket[0]);
  const firstTop = relativeCenters[0] + offset - firstSize.height / 2;
  if (firstTop < WORKFLOW_TIDY_MIN_TOP) {
    offset += WORKFLOW_TIDY_MIN_TOP - firstTop;
  }

  bucket.forEach((node, index) => {
    centerYByNodeId.set(node.id, relativeCenters[index] + offset);
  });
}

function getWorkflowTidyRelativeCenters(bucket: WorkflowNode[]): number[] {
  const centers: number[] = [];
  let nextTop = 0;
  for (const node of bucket) {
    const size = getWorkflowTidyNodeSize(node);
    centers.push(nextTop + size.height / 2);
    nextTop += size.height + WORKFLOW_TIDY_ROW_GAP;
  }
  return centers;
}

function getWorkflowTidyDesiredCenterY({
  nodeId,
  edges,
  getNeighborId,
  centerYByNodeId,
}: {
  nodeId: string;
  edges: WorkflowEdge[];
  getNeighborId: (edge: WorkflowEdge) => string;
  centerYByNodeId: ReadonlyMap<string, number>;
}): number {
  const neighborCenters = edges
    .map((edge) => centerYByNodeId.get(getNeighborId(edge)))
    .filter((center): center is number => typeof center === 'number');
  if (neighborCenters.length === 0) {
    return centerYByNodeId.get(nodeId) ?? WORKFLOW_TIDY_CENTER_Y;
  }
  return (
    neighborCenters.reduce((total, center) => total + center, 0) /
    neighborCenters.length
  );
}

function getWorkflowTidyMedian(values: number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  if (sorted.length % 2 === 1) return sorted[middle] ?? 0;
  return ((sorted[middle - 1] ?? 0) + (sorted[middle] ?? 0)) / 2;
}

function getWorkflowTidyLevelX(
  buckets: ReadonlyMap<number, WorkflowNode[]>,
  sortedLevels: number[]
): Map<number, number> {
  const levelX = new Map<number, number>();
  let nextLevelX = WORKFLOW_TIDY_ORIGIN_X;

  for (const level of sortedLevels) {
    levelX.set(level, nextLevelX);
    nextLevelX +=
      getWorkflowTidyColumnWidth(buckets.get(level) ?? []) +
      WORKFLOW_TIDY_COLUMN_GAP;
  }

  return levelX;
}

function getWorkflowTidyColumnWidth(bucket: WorkflowNode[]): number {
  return Math.max(
    0,
    ...bucket.map((node) => getWorkflowTidyNodeSize(node).width)
  );
}

function getSortedWorkflowTidyLevels(
  buckets: ReadonlyMap<number, WorkflowNode[]>
): number[] {
  return Array.from(buckets.keys()).sort((a, b) => a - b);
}

function compareWorkflowTidyOriginalOrder(
  a: WorkflowNode,
  b: WorkflowNode
): number {
  const ay = a.position?.y ?? WORKFLOW_TIDY_CENTER_Y;
  const by = b.position?.y ?? WORKFLOW_TIDY_CENTER_Y;
  if (ay !== by) return ay - by;

  const ax = a.position?.x ?? WORKFLOW_TIDY_ORIGIN_X;
  const bx = b.position?.x ?? WORKFLOW_TIDY_ORIGIN_X;
  if (ax !== bx) return ax - bx;

  const kindCompare =
    WORKFLOW_TIDY_NODE_KIND_ORDER[a.type] -
    WORKFLOW_TIDY_NODE_KIND_ORDER[b.type];
  if (kindCompare !== 0) return kindCompare;

  return a.id.localeCompare(b.id);
}

function getWorkflowTidyNodeSize(node: WorkflowNode): WorkflowCanvasObjectSize {
  return WORKFLOW_TIDY_NODE_SIZES[node.type];
}

function getWorkflowNodeLevels(
  graph: WorkflowGraph,
  adjacency: WorkflowTidyAdjacency
): Map<string, number> {
  const incomingTargets = new Set(
    adjacency.validEdges.map((edge) => edge.target)
  );
  const starts = graph.nodes.filter(
    (node) => node.type === 'start' || !incomingTargets.has(node.id)
  );
  const levels = new Map<string, number>();
  const queue = starts.length > 0 ? starts.map((node) => node.id) : [];

  for (const start of queue) {
    levels.set(start, 0);
  }

  let guard = graph.nodes.length * Math.max(graph.edges.length, 1);
  while (queue.length > 0 && guard > 0) {
    guard -= 1;
    const sourceId = queue.shift();
    if (!sourceId) continue;
    const sourceLevel = levels.get(sourceId) ?? 0;
    for (const edge of adjacency.outgoing.get(sourceId) ?? []) {
      const nextLevel = Math.min(sourceLevel + 1, graph.nodes.length);
      if ((levels.get(edge.target) ?? -1) >= nextLevel) continue;
      levels.set(edge.target, nextLevel);
      queue.push(edge.target);
    }
  }

  const fallbackLevel =
    Math.max(0, ...Array.from(levels.values())) + (levels.size > 0 ? 1 : 0);
  graph.nodes.forEach((node, index) => {
    if (!levels.has(node.id)) {
      const upstreamLevel = Math.max(
        -1,
        ...(adjacency.incoming.get(node.id) ?? [])
          .map((edge) => levels.get(edge.source))
          .filter((level): level is number => typeof level === 'number')
      );
      levels.set(
        node.id,
        upstreamLevel >= 0 ? upstreamLevel + 1 : fallbackLevel + index
      );
    }
  });

  return levels;
}
