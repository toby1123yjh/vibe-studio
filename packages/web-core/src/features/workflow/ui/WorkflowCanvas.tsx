import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  type DragEvent,
  type MouseEvent,
} from 'react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { useTheme } from '@/shared/hooks/useTheme';
import { AgentIcon } from '@/shared/components/AgentIcon';
import { getAgentBrandAttributes } from '@/shared/lib/agentProviders';
import { useReducedMotion } from '@/shared/hooks/useReducedMotion';
import {
  ReactFlow,
  BaseEdge,
  Background,
  BackgroundVariant,
  Controls,
  EdgeLabelRenderer,
  Handle,
  Position,
  ConnectionLineType,
  ConnectionMode,
  MarkerType,
  NodeResizer,
  SelectionMode,
  useNodesState,
  useEdgesState,
  useUpdateNodeInternals,
  applyNodeChanges,
  applyEdgeChanges,
  useReactFlow,
  getSmoothStepPath,
  type Connection,
  type Edge as ReactFlowEdge,
  type Node as ReactFlowNode,
  type NodeChange,
  type EdgeChange,
  type EdgeProps,
  type DefaultEdgeOptions,
  type OnConnectEnd,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import {
  toReactFlowEdges,
  isWorkflowNodeKind,
  WORKFLOW_REACT_FLOW_EDGE_TYPE,
  type WorkflowGraph,
  type WorkflowNodeKind,
  type WorkflowNodeData,
  type WorkflowNodePosition,
  type WorkflowCanvasObjectNodeData,
  type WorkflowCanvasObjectSize,
  type WorkflowCanvasReactFlowNodeData,
  type WorkflowCanvasReactFlowNodeKind,
  type ReactFlowWorkflowEdgeData,
  WORKFLOW_NODE_DRAG_DATA_TYPE,
  toReactFlowCanvasNodes,
  fromReactFlowCanvasGraph,
} from '../model/workflowGraph';
import {
  WORKFLOW_SEMANTIC_HANDLE_IDS,
  getWorkflowNodeSourceHandles,
  validateWorkflowConnection,
} from '../model/workflowAuthoring';
import {
  getWorkflowEdgeVisual,
  getWorkflowNodeKindLabel,
} from '../model/workflowPresentation';
import { getWorkflowNodeIcon } from './workflowNodeIcons';
import type { ValidationIssue } from './WorkflowValidationPanel';
import { cn } from '../../../shared/lib/utils';
import { getWorkflowAgentDisplay } from '../model/workflowAgentDisplay';
import { type WorkflowCanvasEdgeState } from '../model/workflowCanvasVisualState';
import { isWorkflowNodeAuthorable } from '../model/workflowNodeCatalog';
import { AlertTriangle, Trash2 } from 'lucide-react';
import {
  WORKFLOW_CANVAS_CLASS_NAMES,
  WORKFLOW_CANVAS_EDGE_STATE_PATH_CLASSES,
  WORKFLOW_CANVAS_GROUP_COLOR_CLASSES,
  WORKFLOW_CANVAS_NODE_SURFACE_CLASSES,
  WORKFLOW_CANVAS_NOTE_COLOR_CLASSES,
} from './workflowCanvasTokens';

export const WORKFLOW_CANVAS_NODE_ACTIONS = [
  'configure',
  'duplicate',
  'delete',
] as const;

export const WORKFLOW_CANVAS_EDGE_ACTIONS = ['delete-edge'] as const;

export interface WorkflowCanvasProps {
  graph: WorkflowGraph;
  validationIssues?: ValidationIssue[];
  selectedNodeId?: string | null;
  selectedNodeIds?: string[];
  selectedEdgeId?: string | null;
  readOnly?: boolean;
  onChange?: (graph: WorkflowGraph) => void;
  onSelectionChange?: (selection: WorkflowCanvasSelection) => void;
  onNodeDrop?: (kind: WorkflowNodeKind, position: WorkflowNodePosition) => void;
  onNodeOpen?: (nodeId: string) => void;
  onNodeEdit?: (nodeId: string) => void;
  onNodeDelete?: (nodeId: string) => void | Promise<void>;
  onNodesDelete?: (nodeIds: string[]) => void | Promise<void>;
  onNodesMove?: (positions: Record<string, WorkflowNodePosition>) => void;
  onSplitEdgeWithNode?: (input: {
    edgeId: string;
    nodeId: string;
    position: WorkflowNodePosition;
  }) => void;
  onConnectNodes?: (connection: {
    source: string;
    sourceHandle: string;
    target: string;
  }) => void;
  onConnectDrop?: (connection: {
    source: string;
    sourceHandle: string;
    position: WorkflowNodePosition;
    anchorPoint: { x: number; y: number };
  }) => void;
  onReconnectEdge?: (connection: {
    edgeId: string;
    source: string;
    sourceHandle: string;
    target: string;
  }) => void;
  onEdgeDelete?: (edgeId: string) => void;
  onNodeContextMenu?: (event: WorkflowNodeContextMenuEvent) => void;
}

export interface WorkflowCanvasSelection {
  nodeIds: string[];
  nodeId: string | null;
  edgeId: string | null;
}

export function getWorkflowCanvasNodeClickResult({
  nodeId,
  currentNodeIds,
  shiftKey,
  isAuthorableNode,
}: {
  nodeId: string;
  currentNodeIds: string[];
  shiftKey: boolean;
  isAuthorableNode: boolean;
}): {
  selection: WorkflowCanvasSelection;
  shouldEdit: boolean;
} {
  const nextNodeIds =
    shiftKey && isAuthorableNode
      ? currentNodeIds.includes(nodeId)
        ? currentNodeIds.filter((currentNodeId) => currentNodeId !== nodeId)
        : [...currentNodeIds, nodeId]
      : isAuthorableNode
        ? [nodeId]
        : [];

  return {
    selection: {
      nodeIds: nextNodeIds,
      nodeId:
        nextNodeIds.length === 1
          ? nextNodeIds[0]
          : isAuthorableNode
            ? null
            : nodeId,
      edgeId: null,
    },
    shouldEdit:
      isAuthorableNode && nextNodeIds.length === 1 && nextNodeIds[0] === nodeId,
  };
}

export interface WorkflowNodeContextMenuEvent {
  nodeId: string;
  x: number;
  y: number;
}

interface WorkflowCanvasEdgeData extends ReactFlowWorkflowEdgeData {
  semanticLabel?: string;
  visualStatus?: WorkflowCanvasEdgeState;
  connectionIssue?: WorkflowCanvasConnectionIssue;
}

interface WorkflowCanvasObjectActions {
  readOnly?: boolean;
  update?: (
    objectId: string,
    updates: Partial<WorkflowCanvasObjectNodeData>
  ) => void;
  resize?: (objectId: string, size: WorkflowCanvasObjectSize) => void;
  delete?: (objectId: string) => void;
}

interface BaseNodeProps {
  id: string;
  data: WorkflowNodeData;
  type?: WorkflowNodeKind;
  selected?: boolean;
}

interface CanvasObjectNodeProps {
  id: string;
  data: WorkflowCanvasObjectNodeData;
  selected?: boolean;
}

type WorkflowCanvasFlowNode = ReactFlowNode<
  WorkflowCanvasReactFlowNodeData,
  WorkflowCanvasReactFlowNodeKind
>;

export type WorkflowCanvasConnectionIssue =
  | 'missing_endpoint'
  | 'missing_node'
  | 'self_edge'
  | 'end_source'
  | 'start_target';

const getValidationIssues = (data: WorkflowNodeData): ValidationIssue[] => {
  const issues = data.__validationIssues;
  return Array.isArray(issues) ? (issues as ValidationIssue[]) : [];
};

const getCanvasObjectActions = (
  data: WorkflowCanvasObjectNodeData
): WorkflowCanvasObjectActions =>
  (data.__workflowCanvasObjectActions as WorkflowCanvasObjectActions) ?? {};

function stopCanvasObjectAction(event: MouseEvent<HTMLElement>) {
  event.preventDefault();
  event.stopPropagation();
}

const workflowHandleClass = WORKFLOW_CANVAS_NODE_SURFACE_CLASSES.handle;

function renderWorkflowHandles({
  node,
  t,
}: {
  node: { id: string; type: WorkflowNodeKind; data: WorkflowNodeData };
  t: TFunction<'common'>;
}) {
  const handles = getWorkflowNodeSourceHandles(node);
  return (
    <>
      {node.type !== 'start' ? (
        <Handle
          id={WORKFLOW_SEMANTIC_HANDLE_IDS.input}
          type="target"
          position={Position.Left}
          className={cn(workflowHandleClass, 'workflow-handle-visible z-[3]')}
          aria-label={t('workflow.canvas.inputHandle', {
            defaultValue: 'Input',
          })}
        />
      ) : null}
      {handles.map((handle, index) => (
        <Handle
          key={handle.id}
          id={handle.id}
          type="source"
          position={Position.Right}
          style={{ top: `${((index + 1) / (handles.length + 1)) * 100}%` }}
          title={getLocalizedWorkflowHandleLabel(handle, t)}
          aria-label={getLocalizedWorkflowHandleLabel(handle, t)}
          className={cn(workflowHandleClass, 'workflow-handle-visible z-[3]')}
        />
      ))}
    </>
  );
}

function getLocalizedWorkflowHandleLabel(
  handle: ReturnType<typeof getWorkflowNodeSourceHandles>[number],
  t: TFunction<'common'>
): string {
  switch (handle.kind) {
    case 'input':
      return t('workflow.canvas.inputHandle', { defaultValue: 'Input' });
    case 'default':
      return t('workflow.canvas.nextHandle', { defaultValue: 'Next' });
    case 'winner':
      return t('workflow.edges.winner');
    case 'approve':
      return t('workflow.edges.approve');
    case 'reject':
      return t('workflow.edges.reject');
    case 'condition_branch':
      return handle.label;
  }
}

const BaseNode = ({ id, data, type, selected }: BaseNodeProps) => {
  const { t } = useTranslation('common');
  const nodeKind = type ?? 'agent';
  const Icon = getWorkflowNodeIcon(nodeKind);
  const validationIssues = getValidationIssues(data);
  const issueCount = validationIssues.length;
  const structural = nodeKind === 'start' || nodeKind === 'end';
  const agentDisplay =
    nodeKind === 'agent' || nodeKind === 'task_splitter'
      ? getWorkflowAgentDisplay(data)
      : null;
  const authoringCardClasses = cn(
    WORKFLOW_CANVAS_NODE_SURFACE_CLASSES.card,
    'border-secondary',
    selected && 'border-brand ring-1 ring-brand/25'
  );

  if (structural) {
    return (
      <div
        data-testid={`workflow-node-${id}`}
        style={{ pointerEvents: 'all' }}
        className={cn(
          'relative flex min-w-[120px] cursor-grab items-center gap-2 overflow-visible px-3 py-2 text-normal active:cursor-grabbing',
          authoringCardClasses
        )}
      >
        {renderWorkflowHandles({ node: { id, type: nodeKind, data }, t })}
        {issueCount > 0 ? (
          <div
            data-testid={`workflow-node-issue-${id}`}
            title={validationIssues.map((issue) => issue.message).join('\n')}
            className="absolute -top-5 right-0 text-xs text-warning"
          >
            {t('workflow.canvas.needsConfiguration', {
              defaultValue: 'Needs configuration',
            })}
          </div>
        ) : null}
        <div className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full border border-secondary bg-secondary/30 text-low">
          <Icon className="h-3.5 w-3.5" />
        </div>
        <div className="min-w-0">
          <div
            data-testid={`workflow-node-kind-${id}`}
            className="truncate text-sm font-semibold text-high"
          >
            {data.display_name || getWorkflowNodeKindLabel(nodeKind, t)}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div
      data-testid={`workflow-node-${id}`}
      style={{ pointerEvents: 'all' }}
      className={cn(
        'workflow-agent-step-node relative w-[248px] cursor-grab overflow-visible text-high active:cursor-grabbing',
        authoringCardClasses
      )}
    >
      {renderWorkflowHandles({ node: { id, type: nodeKind, data }, t })}

      {issueCount > 0 ? (
        <div
          data-testid={`workflow-node-issue-${id}`}
          title={validationIssues.map((issue) => issue.message).join('\n')}
          className="absolute -top-5 right-0 text-xs text-warning"
        >
          {t('workflow.canvas.needsConfiguration', {
            defaultValue: 'Needs configuration',
          })}
        </div>
      ) : null}
      <div className="flex items-start gap-3 px-3 py-3 pl-4">
        <div
          {...getAgentBrandAttributes(agentDisplay?.executor)}
          className={cn(
            'flex h-8 w-8 shrink-0 items-center justify-center rounded-md border border-secondary bg-secondary/30 text-low',
            agentDisplay?.executor && 'vk-agent-brand-surface'
          )}
        >
          {agentDisplay?.executor ? (
            <AgentIcon agent={agentDisplay.executor} className="h-4 w-4" />
          ) : (
            <Icon className="h-4 w-4" />
          )}
        </div>
        <div className="min-w-0 flex-1">
          <div
            data-testid={`workflow-node-kind-${id}`}
            className="line-clamp-2 text-sm font-semibold leading-5 text-high"
          >
            {data.display_name || type || t('workflow.canvas.nodeFallback')}
          </div>
          {agentDisplay ? (
            <div
              data-testid={`workflow-node-agent-${id}`}
              {...getAgentBrandAttributes(agentDisplay.executor)}
              className="vk-agent-brand-label mt-1 truncate text-xs"
              title={agentDisplay.agentLabel}
            >
              {agentDisplay.agentLabel}
            </div>
          ) : null}
          {nodeKind === 'arena' ? (
            <div className="mt-1 text-xs text-low">
              {t('workflow.metadata.attempts', { defaultValue: 'Candidates' })}:{' '}
              {data.attempts?.length ?? 0}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
};

function WorkflowStickyNoteNode({ id, data, selected }: CanvasObjectNodeProps) {
  const { t } = useTranslation('common');
  const actions = getCanvasObjectActions(data);
  const readOnly = actions.readOnly === true;
  const color = data.color ?? 'amber';

  return (
    <div
      data-testid={`workflow-note-${id}`}
      className={cn(
        WORKFLOW_CANVAS_NODE_SURFACE_CLASSES.note,
        WORKFLOW_CANVAS_NOTE_COLOR_CLASSES[color],
        selected ? 'ring-2 ring-brand/35' : 'hover:border-white/25'
      )}
    >
      <NodeResizer
        isVisible={selected && !readOnly}
        minWidth={220}
        minHeight={120}
        color="hsl(var(--brand))"
        onResizeEnd={(_, params) =>
          actions.resize?.(id, {
            width: params.width,
            height: params.height,
          })
        }
      />
      <div className="mb-2 flex items-center gap-2">
        <input
          className="nodrag nopan min-w-0 flex-1 bg-transparent text-xs font-semibold text-high outline-none placeholder:text-low"
          value={data.title ?? ''}
          placeholder={t('workflow.defaultGraph.noteTitle')}
          readOnly={readOnly}
          onChange={(event) =>
            actions.update?.(id, { title: event.target.value })
          }
          onPointerDown={(event) => event.stopPropagation()}
        />
        <button
          type="button"
          className="nodrag nopan flex h-7 w-7 shrink-0 items-center justify-center rounded border border-white/10 bg-black/20 text-low transition-colors hover:border-error/50 hover:text-error disabled:cursor-not-allowed disabled:opacity-40"
          aria-label={t('workflow.canvas.deleteNote')}
          title={t('workflow.canvas.deleteNote')}
          disabled={readOnly}
          onPointerDown={(event) => event.stopPropagation()}
          onClick={(event) => {
            stopCanvasObjectAction(event);
            actions.delete?.(id);
          }}
        >
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      </div>
      <textarea
        className="nodrag nopan h-[calc(100%-2.25rem)] w-full resize-none bg-transparent text-xs leading-5 text-normal outline-none placeholder:text-low"
        value={data.content ?? ''}
        placeholder={t('workflow.canvas.notePlaceholder')}
        readOnly={readOnly}
        onChange={(event) =>
          actions.update?.(id, { content: event.target.value })
        }
        onPointerDown={(event) => event.stopPropagation()}
      />
    </div>
  );
}

function WorkflowStageGroupNode({ id, data, selected }: CanvasObjectNodeProps) {
  const { t } = useTranslation('common');
  const actions = getCanvasObjectActions(data);
  const readOnly = actions.readOnly === true;
  const color = data.color ?? 'neutral';

  return (
    <div
      data-testid={`workflow-stage-group-${id}`}
      className={cn(
        WORKFLOW_CANVAS_NODE_SURFACE_CLASSES.stageGroup,
        WORKFLOW_CANVAS_GROUP_COLOR_CLASSES[color],
        selected ? 'ring-2 ring-brand/25' : 'hover:border-white/18'
      )}
    >
      <NodeResizer
        isVisible={selected && !readOnly}
        minWidth={360}
        minHeight={170}
        color="hsl(var(--brand))"
        onResizeEnd={(_, params) =>
          actions.resize?.(id, {
            width: params.width,
            height: params.height,
          })
        }
      />
      <div className="nodrag nopan flex max-w-[360px] flex-col gap-1">
        <div className="flex items-center gap-2">
          <input
            className="min-w-0 flex-1 bg-transparent text-xs font-semibold uppercase tracking-normal text-low outline-none placeholder:text-low"
            value={data.title ?? ''}
            placeholder={t('workflow.editor.stage')}
            readOnly={readOnly}
            onChange={(event) =>
              actions.update?.(id, { title: event.target.value })
            }
            onPointerDown={(event) => event.stopPropagation()}
          />
          <button
            type="button"
            className="flex h-7 w-7 shrink-0 items-center justify-center rounded border border-white/10 bg-black/15 text-low transition-colors hover:border-error/50 hover:text-error disabled:cursor-not-allowed disabled:opacity-40"
            aria-label={t('workflow.canvas.deleteStageGroup')}
            title={t('workflow.canvas.deleteStageGroup')}
            disabled={readOnly}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={(event) => {
              stopCanvasObjectAction(event);
              actions.delete?.(id);
            }}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </button>
        </div>
        <textarea
          className="h-10 resize-none bg-transparent text-[11px] leading-4 text-low outline-none placeholder:text-low/70"
          value={data.description ?? ''}
          placeholder={t('workflow.canvas.stageDescriptionPlaceholder')}
          readOnly={readOnly}
          onChange={(event) =>
            actions.update?.(id, { description: event.target.value })
          }
          onPointerDown={(event) => event.stopPropagation()}
        />
      </div>
    </div>
  );
}

const nodeTypes = {
  start: BaseNode,
  end: BaseNode,
  agent: BaseNode,
  task_splitter: BaseNode,
  condition: BaseNode,
  human_gate: BaseNode,
  transform: BaseNode,
  arena: BaseNode,
  sticky_note: WorkflowStickyNoteNode,
  stage_group: WorkflowStageGroupNode,
};

export const WORKFLOW_CANVAS_SNAP_GRID: [number, number] = [15, 15];
export const WORKFLOW_CANVAS_DELETE_KEYS = ['Backspace', 'Delete'];
export const WORKFLOW_CANVAS_EDGE_TYPE = WORKFLOW_REACT_FLOW_EDGE_TYPE;
export const WORKFLOW_CANVAS_CONNECTION_LINE_TYPE =
  ConnectionLineType.SmoothStep;
export const WORKFLOW_CANVAS_CONNECTION_MODE = ConnectionMode.Strict;
export const WORKFLOW_CANVAS_EDGE_INTERACTION_WIDTH = 32;
export const WORKFLOW_CANVAS_RECONNECT_RADIUS = 16;
export const WORKFLOW_CANVAS_DEFAULT_EDGE_OPTIONS = {
  markerEnd: {
    type: MarkerType.ArrowClosed,
    color: 'context-stroke',
    width: 20,
    height: 20,
    markerUnits: 'userSpaceOnUse',
    strokeWidth: 2.2,
  },
  interactionWidth: WORKFLOW_CANVAS_EDGE_INTERACTION_WIDTH,
} satisfies DefaultEdgeOptions;
export const WORKFLOW_CANVAS_READ_ONLY_NODE_CHANGE_TYPES = [
  'select',
  'dimensions',
  'position',
] as const;
export const WORKFLOW_CANVAS_READ_ONLY_EDGE_CHANGE_TYPES = ['select'] as const;
const EMPTY_VALIDATION_ISSUES: ValidationIssue[] = [];

type ReadOnlyNodeChangeType =
  (typeof WORKFLOW_CANVAS_READ_ONLY_NODE_CHANGE_TYPES)[number];
type ReadOnlyEdgeChangeType =
  (typeof WORKFLOW_CANVAS_READ_ONLY_EDGE_CHANGE_TYPES)[number];

export function filterReadOnlyNodeChanges<
  TNode extends ReactFlowNode = ReactFlowNode,
>(changes: NodeChange<TNode>[]): NodeChange<TNode>[] {
  return changes.filter((change) =>
    (WORKFLOW_CANVAS_READ_ONLY_NODE_CHANGE_TYPES as readonly string[]).includes(
      change.type as ReadOnlyNodeChangeType
    )
  );
}

export function filterReadOnlyEdgeChanges<
  TEdge extends ReactFlowEdge = ReactFlowEdge,
>(changes: EdgeChange<TEdge>[]): EdgeChange<TEdge>[] {
  return changes.filter((change) =>
    (WORKFLOW_CANVAS_READ_ONLY_EDGE_CHANGE_TYPES as readonly string[]).includes(
      change.type as ReadOnlyEdgeChangeType
    )
  );
}

export function getWorkflowCanvasConnectionIssue(
  connection: { source?: string | null; target?: string | null },
  nodeTypeById: ReadonlyMap<string, string | null | undefined>
): WorkflowCanvasConnectionIssue | null {
  const source = connection.source;
  const target = connection.target;
  if (!source || !target) return 'missing_endpoint';
  if (!nodeTypeById.has(source) || !nodeTypeById.has(target)) {
    return 'missing_node';
  }
  if (source === target) return 'self_edge';
  if (nodeTypeById.get(source) === 'end') return 'end_source';
  if (nodeTypeById.get(target) === 'start') return 'start_target';
  return null;
}

export function isWorkflowCanvasConnectionAllowed(
  connection: { source?: string | null; target?: string | null },
  nodeTypeById: ReadonlyMap<string, string | null | undefined>
): boolean {
  return getWorkflowCanvasConnectionIssue(connection, nodeTypeById) === null;
}

function getWorkflowCanvasConnectionIssueMessage(
  issue: WorkflowCanvasConnectionIssue,
  connection: { source?: string | null; target?: string | null },
  t: TFunction<'common'>
): string {
  switch (issue) {
    case 'missing_endpoint':
      return t('workflow.canvas.connectionIssues.missingEndpoint', {
        defaultValue: 'Edge is missing a source or target Node.',
      });
    case 'missing_node':
      return t('workflow.canvas.connectionIssues.missingNode', {
        defaultValue: 'Edge references a Node that no longer exists.',
      });
    case 'self_edge':
      return t('workflow.canvas.connectionIssues.selfEdge', {
        nodeId: connection.source ?? '',
        defaultValue: 'Self-edge found on Node {{nodeId}}.',
      });
    case 'end_source':
      return t('workflow.canvas.connectionIssues.endSource', {
        defaultValue: 'End Nodes cannot start outgoing Edges.',
      });
    case 'start_target':
      return t('workflow.canvas.connectionIssues.startTarget', {
        defaultValue: 'Start Nodes cannot receive incoming Edges.',
      });
  }
}

function getWorkflowCanvasNodeTypeById(
  nodes: Array<{ id: string; type?: string | null }>
): Map<string, string | null | undefined> {
  return new Map(nodes.map((node) => [node.id, node.type]));
}

export function getWorkflowSelfLoopPath({
  sourceX,
  sourceY,
  targetX,
  targetY,
}: {
  sourceX: number;
  sourceY: number;
  targetX: number;
  targetY: number;
}): [string, number, number] {
  const leftX = Math.min(sourceX, targetX);
  const rightX = Math.max(sourceX, targetX);
  const controlX = rightX + 96;
  const controlTopY = Math.min(sourceY, targetY) - 76;
  const controlBottomY = Math.max(sourceY, targetY) + 76;
  const path = `M ${sourceX},${sourceY} C ${controlX},${controlTopY} ${controlX},${controlBottomY} ${targetX},${targetY}`;
  return [path, Math.max(leftX + 58, controlX), (sourceY + targetY) / 2];
}

const WorkflowEdge = ({
  id,
  source,
  target,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  markerEnd,
  data,
  selected,
}: EdgeProps<ReactFlowEdge<WorkflowCanvasEdgeData>>) => {
  const { t } = useTranslation('common');
  const workflowType = data?.workflowType ?? 'default';
  const connectionIssue = data?.connectionIssue;
  const isInvalid = Boolean(connectionIssue);
  const visualStatus = isInvalid ? 'failed' : (data?.visualStatus ?? 'idle');
  const isRunning = visualStatus === 'running';
  const visual = getWorkflowEdgeVisual(workflowType, t);
  const statusPathClass = WORKFLOW_CANVAS_EDGE_STATE_PATH_CLASSES[visualStatus];
  const [edgePath, labelX, labelY] =
    source === target
      ? getWorkflowSelfLoopPath({ sourceX, sourceY, targetX, targetY })
      : getSmoothStepPath({
          sourceX,
          sourceY,
          sourcePosition,
          targetX,
          targetY,
          targetPosition,
          borderRadius: 18,
        });
  const connectionIssueMessage = connectionIssue
    ? getWorkflowCanvasConnectionIssueMessage(
        connectionIssue,
        {
          source,
          target,
        },
        t
      )
    : null;
  const semanticLabel = data?.semanticLabel ?? visual.label;
  const semanticLabelX =
    sourceX + (sourcePosition === Position.Left ? -48 : 48);
  const semanticLabelY = sourceY - 18;

  return (
    <>
      <g
        data-testid={`workflow-edge-${id}`}
        className={cn(
          'group workflow-edge-group',
          selected && 'workflow-edge-group-selected',
          isRunning && 'workflow-edge-group-running',
          isInvalid && 'workflow-edge-group-invalid'
        )}
      >
        <BaseEdge
          id={`${id}-track`}
          path={edgePath}
          interactionWidth={0}
          className="workflow-edge-track"
          style={{
            strokeWidth: selected || isInvalid ? 3 : 2.5,
          }}
        />
        <BaseEdge
          id={id}
          path={edgePath}
          markerEnd={markerEnd}
          interactionWidth={WORKFLOW_CANVAS_EDGE_INTERACTION_WIDTH}
          className={cn(
            'workflow-edge-path',
            selected && 'workflow-edge-path-selected',
            selected && visualStatus === 'idle' && !isInvalid
              ? 'stroke-brand'
              : statusPathClass
          )}
          style={{
            strokeWidth:
              visualStatus === 'running'
                ? 2
                : selected || visualStatus === 'failed' || isInvalid
                  ? 2
                  : 1.5,
            opacity:
              visualStatus === 'running'
                ? 1
                : visualStatus === 'idle'
                  ? 0.58
                  : 0.88,
          }}
        />
        <BaseEdge
          id={`${id}-beam`}
          path={edgePath}
          interactionWidth={0}
          className={cn(
            'workflow-edge-beam transition-opacity',
            isRunning && 'workflow-edge-beam-running',
            selected && isRunning && 'opacity-100'
          )}
          style={{ strokeWidth: 2 }}
        />
        {connectionIssueMessage ? (
          <EdgeLabelRenderer>
            <div
              className="nodrag nopan absolute"
              style={{
                transform: `translate(-50%, -50%) translate(${labelX}px, ${
                  labelY - 30
                }px)`,
                pointerEvents: 'none',
              }}
            >
              <span
                data-testid={`workflow-edge-invalid-${id}`}
                title={connectionIssueMessage}
                className="inline-flex h-6 w-6 items-center justify-center rounded-full border border-error/55 bg-error/15 text-error"
              >
                <AlertTriangle className="h-3.5 w-3.5" />
              </span>
            </div>
          </EdgeLabelRenderer>
        ) : null}
      </g>
      {semanticLabel ? (
        <EdgeLabelRenderer>
          <div
            className="nodrag nopan absolute"
            style={{
              transform: `translate(-50%, -50%) translate(${semanticLabelX}px, ${semanticLabelY}px)`,
              pointerEvents: 'none',
            }}
          >
            <span
              data-testid={`workflow-edge-chip-${id}`}
              className={cn(
                'rounded border px-2 py-0.5 text-[10px] font-semibold shadow-sm',
                visual.chipClass
              )}
            >
              {semanticLabel}
            </span>
          </div>
        </EdgeLabelRenderer>
      ) : null}
      <EdgeLabelRenderer>
        <div
          data-workflow-edge-drop-target={id}
          className="pointer-events-none absolute h-14 w-20 -translate-x-1/2 -translate-y-1/2 rounded-lg border border-transparent transition-colors [.react-flow__node.dragging_~_*_&]:border-brand/40"
          style={{
            transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)`,
          }}
        />
      </EdgeLabelRenderer>
    </>
  );
};

export const workflowCanvasEdgeTypes = {
  [WORKFLOW_CANVAS_EDGE_TYPE]: WorkflowEdge,
};

const GRAPH_AFFECTING_NODE_CHANGE_TYPES = new Set([
  'add',
  'remove',
  'replace',
  'position',
]);

const GRAPH_AFFECTING_EDGE_CHANGE_TYPES = new Set(['add', 'remove', 'replace']);

export function hasGraphAffectingNodeChanges(
  changes: Array<{ type: string }>
): boolean {
  return changes.some((change) =>
    GRAPH_AFFECTING_NODE_CHANGE_TYPES.has(change.type)
  );
}

export function hasGraphAffectingEdgeChanges(
  changes: Array<{ type: string }>
): boolean {
  return changes.some((change) =>
    GRAPH_AFFECTING_EDGE_CHANGE_TYPES.has(change.type)
  );
}

function isEditableKeyboardTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tagName = target.tagName.toLowerCase();
  return (
    target.isContentEditable ||
    tagName === 'input' ||
    tagName === 'textarea' ||
    tagName === 'select'
  );
}

export function WorkflowCanvas({
  graph,
  validationIssues = EMPTY_VALIDATION_ISSUES,
  selectedNodeId,
  selectedNodeIds: explicitSelectedNodeIds,
  selectedEdgeId,
  readOnly = false,
  onChange,
  onSelectionChange,
  onNodeDrop,
  onNodeOpen,
  onNodeEdit,
  onNodeDelete,
  onNodesDelete,
  onNodesMove,
  onSplitEdgeWithNode,
  onConnectNodes,
  onConnectDrop,
  onReconnectEdge,
  onEdgeDelete,
  onNodeContextMenu,
}: WorkflowCanvasProps) {
  const selectedNodeIds = useMemo(
    () => explicitSelectedNodeIds ?? (selectedNodeId ? [selectedNodeId] : []),
    [explicitSelectedNodeIds, selectedNodeId]
  );
  const { effectiveTheme } = useTheme();
  const reducedMotion = useReducedMotion();
  const { t } = useTranslation('common');
  const [nodes, setNodes] = useNodesState<WorkflowCanvasFlowNode>([]);
  const [edges, setEdges] = useEdgesState<
    ReactFlowEdge<WorkflowCanvasEdgeData>
  >([]);
  const updateNodeInternals = useUpdateNodeInternals();
  const { screenToFlowPosition } = useReactFlow();
  const lastSelectionRef = useRef<WorkflowCanvasSelection>({
    nodeIds: [],
    nodeId: null,
    edgeId: null,
  });
  // React Flow can publish an empty controlled-selection snapshot immediately
  // after a node/edge handler applies a selection. Keep the explicit choice
  // authoritative until a real non-empty snapshot arrives; pane clicks still
  // clear this override through applySelection.
  const selectionOverrideRef = useRef<WorkflowCanvasSelection | null>(null);
  const nodesRef = useRef<WorkflowCanvasFlowNode[]>([]);
  const edgesRef = useRef<ReactFlowEdge<WorkflowCanvasEdgeData>[]>([]);
  const lastSyncedGraphRef = useRef<WorkflowGraph | null>(null);
  const reconnectingEdgeIdRef = useRef<string | null>(null);
  const nodeIdsKey = nodes.map((node) => node.id).join('|');

  useEffect(() => {
    nodesRef.current = nodes;
  }, [nodes]);

  useEffect(() => {
    edgesRef.current = edges;
  }, [edges]);

  // Custom Node components expose semantic Handles after React Flow's first
  // layout pass. Ask React Flow to measure them once the controlled Nodes are
  // mounted so drag and connect gestures never target an uninitialized Node.
  useEffect(() => {
    if (!nodeIdsKey) return;
    const frame = requestAnimationFrame(() => {
      updateNodeInternals(nodeIdsKey.split('|'));
    });
    return () => cancelAnimationFrame(frame);
  }, [nodeIdsKey, updateNodeInternals]);

  const reportChange = useCallback(
    (
      newNodes: WorkflowCanvasFlowNode[],
      newEdges: ReactFlowEdge<WorkflowCanvasEdgeData>[]
    ) => {
      if (readOnly || !onChange) return;
      onChange(fromReactFlowCanvasGraph(newNodes, newEdges, graph));
    },
    [graph, readOnly, onChange]
  );

  const updateCanvasObjectNode = useCallback(
    (
      objectId: string,
      updates: Partial<WorkflowCanvasObjectNodeData>,
      size?: WorkflowCanvasObjectSize
    ) => {
      if (readOnly) return;
      const next = nodesRef.current.map((node) => {
        if (node.id !== objectId) return node;
        return {
          ...node,
          data: {
            ...node.data,
            ...updates,
            ...(size ? { size } : {}),
          },
          ...(size
            ? {
                style: {
                  ...(node.style ?? {}),
                  width: size.width,
                  height: size.height,
                },
              }
            : {}),
        } satisfies WorkflowCanvasFlowNode;
      });
      nodesRef.current = next;
      setNodes(next);
      reportChange(next, edgesRef.current);
    },
    [readOnly, reportChange, setNodes]
  );

  const deleteCanvasObjectNode = useCallback(
    (objectId: string) => {
      if (readOnly) return;
      const next = nodesRef.current.filter((node) => node.id !== objectId);
      if (next.length === nodesRef.current.length) return;
      nodesRef.current = next;
      setNodes(next);
      reportChange(next, edgesRef.current);
    },
    [readOnly, reportChange, setNodes]
  );

  // Sync incoming graph to internal state
  useEffect(() => {
    const issuesByNodeId = new Map<string, ValidationIssue[]>();
    for (const issue of validationIssues) {
      if (!issue.nodeId) continue;
      const nodeIssues = issuesByNodeId.get(issue.nodeId) ?? [];
      nodeIssues.push(issue);
      issuesByNodeId.set(issue.nodeId, nodeIssues);
    }

    const preserveLocalPositions =
      readOnly && lastSyncedGraphRef.current === graph;
    setNodes(() => {
      const localNodes = new Map(
        nodesRef.current.map((node) => [node.id, node] as const)
      );
      const fallbackPositions = Object.fromEntries(
        graph.nodes.map((node, index) => [
          node.id,
          {
            x: 120 + (index % 4) * 360,
            y: 160 + Math.floor(index / 4) * 190,
          },
        ])
      );
      const nextNodes = toReactFlowCanvasNodes(graph, fallbackPositions).map(
        (n) => {
          if (!isWorkflowNodeKind(String(n.type))) {
            return {
              ...n,
              data: {
                ...n.data,
                __workflowCanvasObjectActions: {
                  readOnly,
                  update: updateCanvasObjectNode,
                  resize: (objectId: string, size: WorkflowCanvasObjectSize) =>
                    updateCanvasObjectNode(objectId, {}, size),
                  delete: deleteCanvasObjectNode,
                } satisfies WorkflowCanvasObjectActions,
              },
              position: preserveLocalPositions
                ? (localNodes.get(n.id)?.position ?? n.position)
                : n.position,
              selected: localNodes.get(n.id)?.selected ?? false,
            } satisfies WorkflowCanvasFlowNode;
          }

          const data = n.data as WorkflowNodeData;
          return {
            ...n,
            data: {
              ...data,
              __validationIssues: issuesByNodeId.get(n.id) ?? [],
            },
            position: preserveLocalPositions
              ? (localNodes.get(n.id)?.position ?? n.position)
              : n.position,
            selected: localNodes.get(n.id)?.selected ?? false,
          } satisfies WorkflowCanvasFlowNode;
        }
      );
      nodesRef.current = nextNodes;
      return nextNodes;
    });
    lastSyncedGraphRef.current = graph;
    const graphEdgeById = new Map(graph.edges.map((edge) => [edge.id, edge]));
    // Graph edits and validation refreshes must not clear interaction state.
    // The selection effect below handles changes to the controlled IDs.
    const selectedEdges = new Set(
      edgesRef.current.filter((edge) => edge.selected).map((edge) => edge.id)
    );
    const nodeById = new Map(graph.nodes.map((node) => [node.id, node]));
    const nextEdges = toReactFlowEdges(graph).map((edge) => {
      const graphEdge = graphEdgeById.get(edge.id);
      const sourceNode = graphEdge ? nodeById.get(graphEdge.source) : undefined;
      const sourceHandle = graphEdge?.source_handle;
      const semanticHandle = sourceNode
        ? getWorkflowNodeSourceHandles(sourceNode).find(
            (handle) => handle.id === sourceHandle
          )
        : undefined;
      const semanticLabel =
        graphEdge?.type !== 'default' && semanticHandle
          ? getLocalizedWorkflowHandleLabel(semanticHandle, t)
          : undefined;
      return {
        ...edge,
        data: {
          workflowType: edge.data?.workflowType ?? 'default',
          semanticLabel,
          visualStatus: 'idle',
          connectionIssue:
            getWorkflowCanvasConnectionIssue(
              edge,
              getWorkflowCanvasNodeTypeById(graph.nodes)
            ) ?? undefined,
        },
        selected: selectedEdges.has(edge.id),
      };
    }) satisfies ReactFlowEdge<WorkflowCanvasEdgeData>[];
    edgesRef.current = nextEdges;
    setEdges(nextEdges);
  }, [
    graph,
    readOnly,
    setNodes,
    setEdges,
    updateCanvasObjectNode,
    deleteCanvasObjectNode,
    t,
    validationIssues,
  ]);

  useEffect(() => {
    setNodes((current) => {
      let changed = false;
      const next = current.map((node) => {
        const selected = isWorkflowNodeKind(String(node.type))
          ? selectedNodeIds.includes(node.id)
          : selectedNodeId === node.id;
        if (node.selected === selected) return node;
        changed = true;
        return { ...node, selected };
      });
      return changed ? next : current;
    });
    setEdges((current) => {
      let changed = false;
      const next = current.map((edge) => {
        const selected = selectedEdgeId === edge.id;
        if (edge.selected === selected) return edge;
        changed = true;
        return { ...edge, selected };
      });
      return changed ? next : current;
    });
  }, [selectedEdgeId, selectedNodeId, selectedNodeIds, setNodes, setEdges]);

  const onNodesChange = useCallback(
    (changes: NodeChange<WorkflowCanvasFlowNode>[]) => {
      const appliedChanges = (
        readOnly ? filterReadOnlyNodeChanges(changes) : changes
      ).filter((change) => change.type !== 'remove');
      if (appliedChanges.length === 0) return;
      const next = applyNodeChanges(
        appliedChanges,
        nodesRef.current
      ) as WorkflowCanvasFlowNode[];
      nodesRef.current = next;
      setNodes(next);
      if (!readOnly) {
        let movedCanvasObject = false;
        for (const change of appliedChanges) {
          if (change.type !== 'position' || change.dragging === true) continue;
          const movedNode = next.find((node) => node.id === change.id);
          if (!movedNode) continue;
          if (!isWorkflowNodeKind(String(movedNode.type))) {
            movedCanvasObject = true;
          }
        }
        if (movedCanvasObject) {
          reportChange(next, edgesRef.current);
        }
      }
    },
    [readOnly, reportChange, setNodes]
  );

  const onNodeDragStop = useCallback(
    (event: MouseEvent, draggedNode: WorkflowCanvasFlowNode) => {
      if (readOnly || !isWorkflowNodeKind(String(draggedNode.type))) return;
      const selectedWorkflowNodeIds = lastSelectionRef.current.nodeIds;
      if (
        selectedWorkflowNodeIds.length === 1 &&
        draggedNode.type !== 'start' &&
        draggedNode.type !== 'end' &&
        onSplitEdgeWithNode
      ) {
        const dropTarget = Array.from(
          document.querySelectorAll<HTMLElement>(
            '[data-workflow-edge-drop-target]'
          )
        ).find((element) => {
          const bounds = element.getBoundingClientRect();
          return (
            event.clientX >= bounds.left &&
            event.clientX <= bounds.right &&
            event.clientY >= bounds.top &&
            event.clientY <= bounds.bottom
          );
        });
        const edgeId = dropTarget?.dataset.workflowEdgeDropTarget;
        if (edgeId) {
          onSplitEdgeWithNode({
            edgeId,
            nodeId: draggedNode.id,
            position: draggedNode.position,
          });
          return;
        }
      }

      const movedNodeIds =
        selectedWorkflowNodeIds.length > 0
          ? selectedWorkflowNodeIds
          : [draggedNode.id];
      const positions = Object.fromEntries(
        nodesRef.current
          .filter(
            (node) =>
              movedNodeIds.includes(node.id) &&
              isWorkflowNodeKind(String(node.type))
          )
          .map((node) => [node.id, node.position])
      );
      if (Object.keys(positions).length > 0) onNodesMove?.(positions);
    },
    [onNodesMove, onSplitEdgeWithNode, readOnly]
  );

  const onNodeDrag = useCallback(
    (_event: MouseEvent, draggedNode: WorkflowCanvasFlowNode) => {
      if (!readOnly) return;
      const currentNode = nodesRef.current.find(
        (node) => node.id === draggedNode.id
      );
      if (
        currentNode &&
        currentNode.position.x === draggedNode.position.x &&
        currentNode.position.y === draggedNode.position.y
      ) {
        return;
      }
      const next = nodesRef.current.map((node) =>
        node.id === draggedNode.id
          ? { ...node, position: draggedNode.position }
          : node
      );
      nodesRef.current = next;
      setNodes(next);
    },
    [readOnly, setNodes]
  );

  const onEdgesChange = useCallback(
    (changes: EdgeChange<ReactFlowEdge<WorkflowCanvasEdgeData>>[]) => {
      const appliedChanges = (
        readOnly ? filterReadOnlyEdgeChanges(changes) : changes
      ).filter((change) => change.type !== 'remove');
      if (appliedChanges.length === 0) return;
      const next = applyEdgeChanges(appliedChanges, edgesRef.current);
      edgesRef.current = next;
      setEdges(next);
    },
    [readOnly, setEdges]
  );

  const emitSelectionChange = useCallback(
    (selection: WorkflowCanvasSelection) => {
      const lastSelection = lastSelectionRef.current;
      if (
        lastSelection.nodeIds.length === selection.nodeIds.length &&
        lastSelection.nodeIds.every(
          (nodeId, index) => nodeId === selection.nodeIds[index]
        ) &&
        lastSelection.nodeId === selection.nodeId &&
        lastSelection.edgeId === selection.edgeId
      ) {
        return;
      }
      lastSelectionRef.current = selection;
      onSelectionChange?.(selection);
    },
    [onSelectionChange]
  );

  const applySelection = useCallback(
    (selection: WorkflowCanvasSelection) => {
      selectionOverrideRef.current =
        selection.nodeIds.length > 0 || selection.nodeId || selection.edgeId
          ? selection
          : null;
      const nextNodes = nodesRef.current.map((node) => ({
        ...node,
        selected: isWorkflowNodeKind(String(node.type))
          ? selection.nodeIds.includes(node.id) ||
            (selection.nodeIds.length === 0 && selection.nodeId === node.id)
          : selection.nodeId === node.id,
      }));
      const nextEdges = edgesRef.current.map((edge) => ({
        ...edge,
        selected: selection.edgeId === edge.id,
      }));
      nodesRef.current = nextNodes;
      edgesRef.current = nextEdges;
      setNodes(nextNodes);
      setEdges(nextEdges);
      emitSelectionChange(selection);
    },
    [emitSelectionChange, setEdges, setNodes]
  );

  useEffect(() => {
    if (readOnly) return;

    const handleKeyDown = (event: KeyboardEvent) => {
      if (!WORKFLOW_CANVAS_DELETE_KEYS.includes(event.key)) return;
      if (isEditableKeyboardTarget(event.target)) return;
      const selection = lastSelectionRef.current;
      if (
        !selection.edgeId &&
        !selection.nodeId &&
        selection.nodeIds.length === 0
      ) {
        return;
      }
      event.preventDefault();
      if (selection.edgeId) {
        onEdgeDelete?.(selection.edgeId);
        return;
      }
      if (selection.nodeIds.length > 0) {
        const deletableNodeIds = selection.nodeIds.filter((nodeId) => {
          const node = nodesRef.current.find(
            (candidate) => candidate.id === nodeId
          );
          return node?.type !== 'start' && node?.type !== 'end';
        });
        if (deletableNodeIds.length > 0) {
          if (onNodesDelete) void onNodesDelete(deletableNodeIds);
          else if (deletableNodeIds.length === 1) {
            void onNodeDelete?.(deletableNodeIds[0]);
          }
        }
        return;
      }
      const selectedNode = nodesRef.current.find(
        (node) => node.id === selection.nodeId
      );
      if (!selectedNode) return;
      if (isWorkflowNodeKind(String(selectedNode.type))) {
        if (selectedNode.type !== 'start' && selectedNode.type !== 'end') {
          void onNodeDelete?.(selectedNode.id);
        }
      } else {
        deleteCanvasObjectNode(selectedNode.id);
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [
    deleteCanvasObjectNode,
    onEdgeDelete,
    onNodeDelete,
    onNodesDelete,
    readOnly,
  ]);

  const onCanvasDoubleClickCapture = useCallback(
    (event: MouseEvent<HTMLDivElement>) => {
      if (!onNodeOpen) return;
      const target = event.target;
      if (!(target instanceof Element)) return;

      const nodeElement = target.closest('.react-flow__node[data-id]');
      const nodeId = nodeElement?.getAttribute('data-id');
      if (!nodeId) return;
      const node = nodesRef.current.find(
        (candidate) => candidate.id === nodeId
      );
      if (
        !node?.type ||
        !isWorkflowNodeKind(String(node.type)) ||
        node.type !== 'agent'
      ) {
        return;
      }

      event.stopPropagation();
      applySelection({ nodeIds: [nodeId], nodeId, edgeId: null });
      onNodeOpen(nodeId);
    },
    [applySelection, onNodeOpen]
  );

  const onConnect = useCallback(
    (connection: Connection) => {
      if (readOnly) return;
      const sourceHandle = connection.sourceHandle ?? '';
      const issue = validateWorkflowConnection(graph, {
        source: connection.source ?? '',
        sourceHandle,
        target: connection.target ?? '',
      });
      if (issue || !connection.source || !connection.target) {
        return;
      }
      onConnectNodes?.({
        source: connection.source,
        sourceHandle,
        target: connection.target,
      });
    },
    [graph, onConnectNodes, readOnly]
  );

  const onConnectEnd = useCallback<OnConnectEnd>(
    (event, connectionState) => {
      if (
        readOnly ||
        !onConnectDrop ||
        connectionState.toNode ||
        !connectionState.fromNode ||
        connectionState.fromHandle?.type !== 'source'
      ) {
        return;
      }
      const pointer =
        'changedTouches' in event ? event.changedTouches.item(0) : event;
      if (!pointer || !connectionState.fromHandle.id) return;
      const anchorPoint = { x: pointer.clientX, y: pointer.clientY };
      onConnectDrop({
        source: connectionState.fromNode.id,
        sourceHandle: connectionState.fromHandle.id,
        position: screenToFlowPosition(anchorPoint),
        anchorPoint,
      });
    },
    [onConnectDrop, readOnly, screenToFlowPosition]
  );

  const onReconnect = useCallback(
    (
      oldEdge: ReactFlowEdge<WorkflowCanvasEdgeData>,
      newConnection: Connection
    ) => {
      if (readOnly) return;
      const sourceHandle = newConnection.sourceHandle ?? '';
      if (
        validateWorkflowConnection(graph, {
          source: newConnection.source ?? '',
          sourceHandle,
          target: newConnection.target ?? '',
          ignoredEdgeId: oldEdge.id,
        })
      ) {
        return;
      }
      if (!newConnection.source || !newConnection.target) return;
      onReconnectEdge?.({
        edgeId: oldEdge.id,
        source: newConnection.source,
        sourceHandle,
        target: newConnection.target,
      });
    },
    [graph, onReconnectEdge, readOnly]
  );

  const onSelectionChangeReactFlow = useCallback(
    ({
      nodes: selectedNodes,
      edges: selectedEdges,
    }: {
      nodes: ReactFlowNode[];
      edges: ReactFlowEdge[];
    }) => {
      if (
        selectedNodes.length === 0 &&
        selectedEdges.length === 0 &&
        selectionOverrideRef.current
      ) {
        return;
      }
      const hasSingleNode = selectedNodes.length === 1;
      const hasSingleEdge = selectedEdges.length === 1;
      const workflowNodeIds = selectedNodes
        .filter(
          (node) =>
            isWorkflowNodeKind(String(node.type)) &&
            node.type !== 'start' &&
            node.type !== 'end'
        )
        .map((node) => node.id);
      emitSelectionChange({
        nodeIds: hasSingleEdge ? [] : workflowNodeIds,
        nodeId:
          hasSingleNode && !hasSingleEdge && workflowNodeIds.length <= 1
            ? selectedNodes[0].id
            : null,
        edgeId:
          hasSingleEdge && workflowNodeIds.length === 0
            ? selectedEdges[0].id
            : null,
      });
    },
    [emitSelectionChange]
  );

  const onDragOver = useCallback(
    (event: DragEvent<HTMLDivElement>) => {
      if (readOnly) return;
      if (!event.dataTransfer.types.includes(WORKFLOW_NODE_DRAG_DATA_TYPE)) {
        return;
      }
      event.preventDefault();
      event.dataTransfer.dropEffect = 'copy';
    },
    [readOnly]
  );

  const onDrop = useCallback(
    (event: DragEvent<HTMLDivElement>) => {
      if (readOnly || !onNodeDrop) return;

      const nodeKind = event.dataTransfer.getData(WORKFLOW_NODE_DRAG_DATA_TYPE);
      if (
        !isWorkflowNodeKind(nodeKind) ||
        !isWorkflowNodeAuthorable(nodeKind)
      ) {
        return;
      }

      event.preventDefault();
      onNodeDrop(
        nodeKind,
        screenToFlowPosition({ x: event.clientX, y: event.clientY })
      );
    },
    [onNodeDrop, readOnly, screenToFlowPosition]
  );

  return (
    <div
      className={WORKFLOW_CANVAS_CLASS_NAMES.root}
      data-workflow-canvas-root="true"
      tabIndex={-1}
      onDoubleClickCapture={onCanvasDoubleClickCapture}
    >
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        edgeTypes={workflowCanvasEdgeTypes}
        onNodesChange={onNodesChange}
        onNodeDrag={onNodeDrag}
        onNodeDragStop={onNodeDragStop}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        onConnectEnd={onConnectEnd}
        onReconnect={onReconnect}
        onReconnectStart={(_, edge) => {
          reconnectingEdgeIdRef.current = edge.id;
        }}
        onReconnectEnd={(_, edge) => {
          if (reconnectingEdgeIdRef.current === edge.id) {
            reconnectingEdgeIdRef.current = null;
          }
        }}
        onSelectionChange={onSelectionChangeReactFlow}
        onNodeClick={(event, node) => {
          const isAuthorableNode =
            isWorkflowNodeKind(String(node.type)) &&
            node.type !== 'start' &&
            node.type !== 'end';
          const clickResult = getWorkflowCanvasNodeClickResult({
            nodeId: node.id,
            currentNodeIds: lastSelectionRef.current.nodeIds,
            shiftKey: event.shiftKey,
            isAuthorableNode,
          });
          applySelection(clickResult.selection);
          if (
            event.target instanceof Element &&
            event.target.closest('.react-flow__handle')
          ) {
            return;
          }
          if (
            node.type &&
            isWorkflowNodeKind(String(node.type)) &&
            node.type !== 'start' &&
            node.type !== 'end' &&
            clickResult.shouldEdit
          ) {
            onNodeEdit?.(node.id);
          }
        }}
        onNodeDoubleClick={(_, node) => {
          if (
            !node.type ||
            !isWorkflowNodeKind(String(node.type)) ||
            node.type !== 'agent'
          ) {
            return;
          }
          applySelection({ nodeIds: [node.id], nodeId: node.id, edgeId: null });
          onNodeOpen?.(node.id);
        }}
        onNodeContextMenu={(event, node) => {
          if (
            !node.type ||
            !isWorkflowNodeKind(String(node.type)) ||
            node.type === 'start' ||
            node.type === 'end'
          ) {
            return;
          }
          event.preventDefault();
          applySelection({ nodeIds: [node.id], nodeId: node.id, edgeId: null });
          onNodeContextMenu?.({
            nodeId: node.id,
            x: event.clientX,
            y: event.clientY,
          });
        }}
        onEdgeClick={(_, edge) =>
          applySelection({ nodeIds: [], nodeId: null, edgeId: edge.id })
        }
        onPaneClick={() =>
          applySelection({ nodeIds: [], nodeId: null, edgeId: null })
        }
        onDragOver={onDragOver}
        onDrop={onDrop}
        isValidConnection={(connection) =>
          validateWorkflowConnection(graph, {
            source: connection.source ?? '',
            sourceHandle: connection.sourceHandle ?? '',
            target: connection.target ?? '',
            ignoredEdgeId: reconnectingEdgeIdRef.current ?? undefined,
          }) === null
        }
        defaultEdgeOptions={WORKFLOW_CANVAS_DEFAULT_EDGE_OPTIONS}
        // Read-only canvases keep local drag feedback, but onNodeDragStop
        // deliberately skips the persistence callback.
        nodesDraggable={true}
        nodesConnectable={!readOnly}
        edgesReconnectable={!readOnly}
        reconnectRadius={WORKFLOW_CANVAS_RECONNECT_RADIUS}
        elevateEdgesOnSelect
        connectionMode={WORKFLOW_CANVAS_CONNECTION_MODE}
        elementsSelectable={true}
        selectionMode={SelectionMode.Partial}
        selectionKeyCode="Shift"
        multiSelectionKeyCode="Shift"
        connectionLineType={WORKFLOW_CANVAS_CONNECTION_LINE_TYPE}
        connectionLineStyle={{
          stroke: 'hsl(var(--brand))',
          strokeWidth: 2,
          strokeDasharray: '8 8',
        }}
        snapToGrid
        snapGrid={WORKFLOW_CANVAS_SNAP_GRID}
        deleteKeyCode={null}
        fitView
        colorMode={effectiveTheme}
        className={WORKFLOW_CANVAS_CLASS_NAMES.reactFlow}
      >
        <Background
          id="bg-grid-dots"
          variant={BackgroundVariant.Dots}
          gap={WORKFLOW_CANVAS_SNAP_GRID[0]}
          size={1}
          color="var(--workflow-canvas-grid-dot)"
        />
        <Controls
          className={WORKFLOW_CANVAS_CLASS_NAMES.controls}
          fitViewOptions={{ duration: reducedMotion ? 0 : 240 }}
        />
      </ReactFlow>
    </div>
  );
}
