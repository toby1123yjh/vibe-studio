import { useTheme } from '@/shared/hooks/useTheme';
import { useReducedMotion } from '@/shared/hooks/useReducedMotion';
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import {
  Activity,
  AlertCircle,
  CheckCircle,
  Clock,
  ExternalLink,
  History,
  Swords,
  User,
} from 'lucide-react';
import {
  Background,
  BackgroundVariant,
  ConnectionMode,
  Controls,
  Handle,
  Position,
  ReactFlow,
  type Edge as ReactFlowEdge,
  type Node as ReactFlowNode,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import type {
  WorkflowNodeExecutionResponse,
  WorkflowNodeWorkStatus,
  WorkflowNodeWorkView,
  WorkflowRunResponse,
} from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import { Checkbox } from '@vibe/ui/components/Checkbox';
import {
  FloatingPanel,
  FloatingPanelBody,
  FloatingPanelDescription,
  FloatingPanelFooter,
  FloatingPanelHeader,
  FloatingPanelTitle,
} from '@vibe/ui/components/FloatingPanel';
import { AgentIcon } from '@/shared/components/AgentIcon';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useWorkflowRunMutations } from '@/shared/hooks/useWorkflowRun';
import { useWorkflowTemplate } from '@/shared/hooks/useWorkflowTemplates';
import { buildWorkspaceSessionHref } from '@/shared/lib/routes/workspaceRoutes';
import { cn } from '@/shared/lib/utils';
import {
  getConditionRouterHumanPrompt,
  getConditionRouterReason,
  parseConditionRouterOutput,
} from '../model/workflowConditionRouterOutput';
import {
  WORKFLOW_SEMANTIC_HANDLE_IDS,
  getWorkflowNodeSourceHandles,
} from '../model/workflowAuthoring';
import {
  migrateWorkflowGraph,
  toReactFlowEdges,
  toReactFlowNodes,
  type WorkflowGraph,
  type WorkflowNode,
  type WorkflowNodeData,
  type WorkflowNodeKind,
} from '../model/workflowGraph';
import { getWorkflowAgentDisplay } from '../model/workflowAgentDisplay';
import {
  getWorkflowCanvasEdgeState,
  getWorkflowCanvasNodeState,
} from '../model/workflowCanvasVisualState';
import { getWorkflowNodeKindLabel } from '../model/workflowPresentation';
import { consumeWorkflowRunNodeFocus } from '../model/workflowRunNodeFocus';
import {
  getWorkflowNodeActionGate,
  getWorkflowNodeExecutionForWork,
  getWorkflowNodeExecutionTarget,
  getWorkflowNodeWork,
  getWorkflowRuntimeAttentionItems,
  getWorkflowRuntimeView,
  getWorkflowSplitBatchesForNode,
  getWorkflowSplitNodeProgress,
  type WorkflowSplitProgress,
  type WorkflowRuntimeAttentionItem,
  type WorkflowRuntimeAuthority,
} from '../model/workflowRuntimeView';
import {
  WORKFLOW_CANVAS_CLASS_NAMES,
  WORKFLOW_CANVAS_COLOR_TOKENS,
  WORKFLOW_CANVAS_NODE_STATE_DOT_CLASSES,
  WORKFLOW_RUN_NODE_STATE_CHIP_CLASSES,
  WORKFLOW_CANVAS_NODE_SURFACE_CLASSES,
  WORKFLOW_CANVAS_NODE_STATE_FRAME_CLASSES,
} from './workflowCanvasTokens';
import {
  WORKFLOW_CANVAS_DEFAULT_EDGE_OPTIONS,
  workflowCanvasEdgeTypes,
} from './WorkflowCanvas';
import { workflowNodeStatusKey } from './workflowI18n';
import { getWorkflowNodeIcon } from './workflowNodeIcons';
import { WorkflowSplitBatchDetails } from './WorkflowSplitBatchDetails';
import { WorkflowFileReferences } from './WorkflowFileReferences';

export interface WorkflowRunCanvasTabProps {
  projectId: string;
  run: WorkflowRunResponse;
}

interface RunNodeData extends WorkflowNodeData {
  runtimeStatus?: WorkflowNodeWorkStatus;
  runtimeAuthority?: WorkflowRuntimeAuthority;
  nodeId: string;
  nodeType: WorkflowNodeKind;
  splitProgress?: WorkflowSplitProgress | null;
  awaitingDispatch?: boolean;
}

interface ConditionBranchOption {
  targetNodeId: string;
  targetLabel: string;
  condition: string | null;
}

interface GateSubmission {
  nodeId: string;
  executionId: string;
  action: 'approve' | 'reject';
  phase: 'submitting' | 'awaiting-projection';
}

interface ConditionSubmission {
  nodeId: string;
  executionId: string;
  phase: 'submitting' | 'awaiting-projection';
}

const nodeTypes = {
  start: RunNode,
  end: RunNode,
  agent: RunNode,
  task_splitter: RunNode,
  condition: RunNode,
  human_gate: RunNode,
  transform: RunNode,
  arena: RunNode,
};

function RunNode({ data }: { data: RunNodeData }) {
  const { t } = useTranslation('common');
  const type = data.nodeType;
  const structural = type === 'start' || type === 'end';
  const Icon = getWorkflowNodeIcon(type);
  const nodeState = data.awaitingDispatch
    ? 'waiting'
    : getWorkflowCanvasNodeState({
        data,
        executionStatus: data.runtimeStatus,
        nodeType: type,
      });
  const stateLabel = data.awaitingDispatch
    ? t('workflow.split.waitingDispatch')
    : data.runtimeStatus
      ? t(`workflow.nodeStatus.${workflowNodeStatusKey(data.runtimeStatus)}`)
      : t('workflow.runCanvas.notExecutedYet');
  const agentDisplay =
    type === 'agent' || type === 'task_splitter'
      ? getWorkflowAgentDisplay(data)
      : null;
  const sourceHandles = getWorkflowNodeSourceHandles({
    id: data.nodeId,
    type,
    data,
  });
  const cardClasses = cn(
    WORKFLOW_CANVAS_NODE_SURFACE_CLASSES.card,
    WORKFLOW_CANVAS_NODE_STATE_FRAME_CLASSES[nodeState]
  );

  const handles = (
    <>
      {type !== 'start' ? (
        <Handle
          id={WORKFLOW_SEMANTIC_HANDLE_IDS.input}
          type="target"
          position={Position.Left}
          className="opacity-0"
        />
      ) : null}
      {sourceHandles.map((handle, index) => (
        <Handle
          key={handle.id}
          id={handle.id}
          type="source"
          position={Position.Right}
          style={{
            top: `${((index + 1) / (sourceHandles.length + 1)) * 100}%`,
          }}
          className="opacity-0"
        />
      ))}
    </>
  );

  if (structural) {
    return (
      <div
        data-testid={`workflow-run-node-${data.nodeId}`}
        className={cn(
          'relative flex min-w-[120px] items-center gap-2 overflow-visible px-3 py-2 text-normal',
          cardClasses
        )}
      >
        {handles}
        <span
          title={stateLabel}
          aria-label={stateLabel}
          className={cn(
            'absolute right-2 top-2 h-2.5 w-2.5 rounded-full border border-panel',
            WORKFLOW_CANVAS_NODE_STATE_DOT_CLASSES[nodeState]
          )}
        />
        <div className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full border border-secondary bg-secondary/30 text-low">
          <Icon className="h-3.5 w-3.5" />
        </div>
        <div className="min-w-0">
          <div className="truncate pr-3 text-sm font-semibold text-high">
            {data.display_name || getWorkflowNodeKindLabel(type, t)}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div
      data-testid={`workflow-run-node-${data.nodeId}`}
      className={cn(
        'relative w-[248px] overflow-visible text-high',
        cardClasses
      )}
    >
      {handles}
      <span
        className={cn(
          'absolute right-3 top-3 h-2.5 w-2.5 rounded-full border border-panel shadow-sm',
          WORKFLOW_CANVAS_NODE_STATE_DOT_CLASSES[nodeState]
        )}
        title={stateLabel}
      />
      <div className="flex items-start gap-3 px-3 py-3 pl-4">
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md border border-secondary bg-secondary/30 text-low">
          {agentDisplay?.executor ? (
            <AgentIcon agent={agentDisplay.executor} className="h-4 w-4" />
          ) : (
            <Icon className="h-4 w-4" />
          )}
        </div>
        <div className="min-w-0 flex-1">
          <div className="line-clamp-2 pr-3 text-sm font-semibold leading-5 text-high">
            {data.display_name || getWorkflowNodeKindLabel(type, t)}
          </div>
          {agentDisplay ? (
            <div className="mt-1 truncate text-xs text-low">
              {agentDisplay.agentLabel}
            </div>
          ) : null}
        </div>
      </div>
      <div className="border-t border-secondary/60 px-3 py-2 pl-4">
        <span
          className={cn(
            'inline-flex items-center rounded border px-1.5 py-0.5 text-xs font-medium leading-none',
            WORKFLOW_RUN_NODE_STATE_CHIP_CLASSES[nodeState]
          )}
        >
          {stateLabel}
        </span>
        {data.splitProgress ? (
          <p className="mt-half text-xs text-low">
            {t('workflow.split.progress', { ...data.splitProgress })}
          </p>
        ) : null}
      </div>
    </div>
  );
}

function parseWorkflowGraph(graphJson: string): WorkflowGraph | null {
  try {
    const parsed = JSON.parse(graphJson) as WorkflowGraph;
    if (parsed && Array.isArray(parsed.nodes) && Array.isArray(parsed.edges)) {
      return migrateWorkflowGraph(parsed);
    }
  } catch {
    return null;
  }
  return null;
}

function getDefaultPositions(graph: WorkflowGraph) {
  return Object.fromEntries(
    graph.nodes.map((node, index) => [
      node.id,
      {
        x: 80 + (index % 4) * 320,
        y: 80 + Math.floor(index / 4) * 150,
      },
    ])
  );
}

function buildRunWorkspaceHref(
  projectId: string,
  run: WorkflowRunResponse
): string | null {
  return run.workspace_id
    ? `/projects/${projectId}/tasks/${run.task_id}/workspaces/${run.workspace_id}`
    : null;
}

function buildConditionBranchOptions(
  graph: WorkflowGraph,
  conditionNode: WorkflowNode,
  t: TFunction<'common'>
): ConditionBranchOption[] {
  if (conditionNode.type !== 'condition') return [];

  const branchByTarget = new Map(
    (conditionNode.data.branches ?? [])
      .filter((branch) => branch.target_node_id)
      .map((branch) => [branch.target_node_id as string, branch])
  );
  const nodeById = new Map(graph.nodes.map((node) => [node.id, node]));
  const seenTargets = new Set<string>();
  const options: ConditionBranchOption[] = [];

  for (const edge of graph.edges) {
    if (edge.source !== conditionNode.id || seenTargets.has(edge.target)) {
      continue;
    }
    seenTargets.add(edge.target);

    const targetNode = nodeById.get(edge.target);
    const branch = branchByTarget.get(edge.target);
    const condition = branch?.condition?.trim();
    options.push({
      targetNodeId: edge.target,
      targetLabel:
        targetNode?.data.display_name?.trim() ||
        (targetNode
          ? getWorkflowNodeKindLabel(targetNode.type, t)
          : edge.target),
      condition: condition && condition.length > 0 ? condition : null,
    });
  }

  return options;
}

export function WorkflowRunCanvasTab({
  projectId,
  run,
}: WorkflowRunCanvasTabProps) {
  const { effectiveTheme } = useTheme();
  const reducedMotion = useReducedMotion();
  const { t } = useTranslation('common');
  const { data: template, isLoading: isTemplateLoading } = useWorkflowTemplate(
    run.workflow_id
  );
  const navigation = useAppNavigation();
  const {
    approveNode,
    rejectNode,
    selectConditionBranch,
    isApproving,
    isRejecting,
    isSelectingConditionBranch,
  } = useWorkflowRunMutations();
  const gateLockRef = useRef(false);
  const conditionLockRef = useRef(false);
  const consumedQueuedFocusForRunRef = useRef<string | null>(null);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [detailsNodeId, setDetailsNodeId] = useState<string | null>(null);
  const [gateSubmission, setGateSubmission] = useState<GateSubmission | null>(
    null
  );
  const [conditionSubmission, setConditionSubmission] =
    useState<ConditionSubmission | null>(null);
  const [actionError, setActionError] = useState<{
    nodeId: string;
    message: string;
  } | null>(null);
  const runtimeView = useMemo(() => getWorkflowRuntimeView(run), [run]);
  const graph = useMemo(
    () => (template ? parseWorkflowGraph(template.graph_json) : null),
    [template]
  );
  const attentionItems = useMemo(
    () => getWorkflowRuntimeAttentionItems(runtimeView),
    [runtimeView]
  );

  const openNodeDetails = useCallback((nodeId: string) => {
    setActionError(null);
    setSelectedNodeId(nodeId);
    setDetailsNodeId(nodeId);
  }, []);

  useEffect(() => {
    if (consumedQueuedFocusForRunRef.current === run.id) return;
    consumedQueuedFocusForRunRef.current = run.id;

    const queuedFocus = consumeWorkflowRunNodeFocus(run.id);
    if (queuedFocus) openNodeDetails(queuedFocus.nodeId);
  }, [openNodeDetails, run.id]);

  useEffect(() => {
    if (!graph) return;
    if (
      selectedNodeId &&
      !graph.nodes.some((node) => node.id === selectedNodeId)
    ) {
      setSelectedNodeId(null);
    }
    if (
      detailsNodeId &&
      !graph.nodes.some((node) => node.id === detailsNodeId)
    ) {
      setDetailsNodeId(null);
    }
  }, [detailsNodeId, graph, selectedNodeId]);

  useEffect(() => {
    if (!gateSubmission) return;
    const work = getWorkflowNodeWork(runtimeView, gateSubmission.nodeId);
    const execution = getWorkflowNodeExecutionForWork(run, work);
    const gate = getWorkflowNodeActionGate(work);
    if (
      execution?.id !== gateSubmission.executionId ||
      (!gate.canApprove && !gate.canReject)
    ) {
      setGateSubmission(null);
    }
  }, [gateSubmission, run, runtimeView]);

  useEffect(() => {
    if (!conditionSubmission) return;
    const work = getWorkflowNodeWork(runtimeView, conditionSubmission.nodeId);
    const execution = getWorkflowNodeExecutionForWork(run, work);
    if (
      execution?.id !== conditionSubmission.executionId ||
      !getWorkflowNodeActionGate(work).canSelectConditionBranch
    ) {
      setConditionSubmission(null);
    }
  }, [conditionSubmission, run, runtimeView]);

  const workMap = useMemo(
    () => new Map(runtimeView.node_work.map((work) => [work.node_id, work])),
    [runtimeView]
  );
  const nodes = useMemo(() => {
    if (!graph) return [];
    return toReactFlowNodes(graph, getDefaultPositions(graph)).map((node) => {
      const work = workMap.get(node.id);
      return {
        ...node,
        selected: node.id === selectedNodeId,
        data: {
          ...node.data,
          runtimeStatus: work?.status,
          runtimeAuthority: work?.runtime_authority,
          nodeId: node.id,
          nodeType: node.type,
          splitProgress: getWorkflowSplitNodeProgress(run, node.id),
          awaitingDispatch: run.split_batches.some(
            (batch) =>
              batch.splitter_node_id === node.id &&
              batch.state === 'pending' &&
              !batch.dispatch_path
          ),
        },
      } satisfies ReactFlowNode<RunNodeData, WorkflowNodeKind>;
    });
  }, [graph, run, selectedNodeId, workMap]);
  const edges = useMemo(() => {
    if (!graph) return [];
    return toReactFlowEdges(graph).map((edge) => ({
      ...edge,
      data: {
        ...edge.data,
        visualStatus: getWorkflowCanvasEdgeState(
          workMap.get(edge.source)?.status,
          workMap.get(edge.target)?.status
        ),
      },
    })) satisfies ReactFlowEdge[];
  }, [graph, workMap]);

  const detailsWork = getWorkflowNodeWork(runtimeView, detailsNodeId);
  const detailsExecution = getWorkflowNodeExecutionForWork(run, detailsWork);
  const detailsGraphNode =
    graph?.nodes.find((node) => node.id === detailsNodeId) ?? null;
  const gatePending = Boolean(gateSubmission) || isApproving || isRejecting;
  const conditionPending =
    Boolean(conditionSubmission) || isSelectingConditionBranch;

  const handleHumanGateDecision = async (action: 'approve' | 'reject') => {
    if (gateLockRef.current || gateSubmission || !detailsNodeId) return;

    const work = getWorkflowNodeWork(runtimeView, detailsNodeId);
    const execution = getWorkflowNodeExecutionForWork(run, work);
    const gate = getWorkflowNodeActionGate(work);
    const allowed = action === 'approve' ? gate.canApprove : gate.canReject;
    if (!execution || !allowed) {
      setActionError({
        nodeId: detailsNodeId,
        message: t('workflow.runCanvas.actionUnavailable'),
      });
      return;
    }

    gateLockRef.current = true;
    setActionError(null);
    setGateSubmission({
      nodeId: detailsNodeId,
      executionId: execution.id,
      action,
      phase: 'submitting',
    });
    try {
      if (action === 'approve') {
        await approveNode({
          runId: run.id,
          nodeId: execution.node_id,
          payload: { node_execution_id: execution.id },
        });
      } else {
        await rejectNode({
          runId: run.id,
          nodeId: execution.node_id,
          payload: { node_execution_id: execution.id },
        });
      }
      setGateSubmission((current) =>
        current?.executionId === execution.id
          ? { ...current, phase: 'awaiting-projection' }
          : current
      );
    } catch (decisionError) {
      setGateSubmission(null);
      setActionError({
        nodeId: detailsNodeId,
        message:
          decisionError instanceof Error
            ? decisionError.message
            : t(
                action === 'approve'
                  ? 'workflow.dashboard.approveFailed'
                  : 'workflow.dashboard.rejectFailed'
              ),
      });
    } finally {
      gateLockRef.current = false;
    }
  };

  const handleSelectConditionBranch = async (targetNodeIds: string[]) => {
    if (
      conditionLockRef.current ||
      conditionSubmission ||
      isSelectingConditionBranch ||
      !detailsNodeId
    ) {
      return;
    }
    const work = getWorkflowNodeWork(runtimeView, detailsNodeId);
    const execution = getWorkflowNodeExecutionForWork(run, work);
    if (
      !execution ||
      !getWorkflowNodeActionGate(work).canSelectConditionBranch
    ) {
      setActionError({
        nodeId: detailsNodeId,
        message: t('workflow.runCanvas.actionUnavailable'),
      });
      return;
    }

    conditionLockRef.current = true;
    setActionError(null);
    setConditionSubmission({
      nodeId: detailsNodeId,
      executionId: execution.id,
      phase: 'submitting',
    });
    try {
      await selectConditionBranch({
        runId: run.id,
        nodeId: execution.node_id,
        payload: {
          selected_target_node_ids: targetNodeIds,
          node_execution_id: execution.id,
        },
      });
      setConditionSubmission((current) =>
        current?.executionId === execution.id
          ? { ...current, phase: 'awaiting-projection' }
          : current
      );
    } catch (branchError) {
      setConditionSubmission(null);
      setActionError({
        nodeId: detailsNodeId,
        message:
          branchError instanceof Error
            ? branchError.message
            : t('workflow.runCanvas.conditionBranchFailed'),
      });
    } finally {
      conditionLockRef.current = false;
    }
  };

  if (isTemplateLoading) {
    return (
      <div className="flex h-full items-center justify-center text-low">
        <Activity className="mr-2 h-4 w-4 animate-spin motion-reduce:animate-none" />
        {t('workflow.runCanvas.loadingGraph')}
      </div>
    );
  }

  if (!graph) {
    return (
      <div className="flex h-full items-center justify-center p-base text-sm text-error">
        {t('workflow.runCanvas.loadGraphFailed')}
      </div>
    );
  }

  const attentionItem = detailsNodeId ? null : (attentionItems[0] ?? null);

  return (
    <div className="workflow-canvas-shell workflow-canvas-surface relative h-full min-h-[360px] w-full overflow-hidden bg-[var(--workflow-canvas-bg)]">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        edgeTypes={workflowCanvasEdgeTypes}
        onNodeClick={(_, node) => openNodeDetails(node.id)}
        onNodeDoubleClick={(_, node) => openNodeDetails(node.id)}
        onPaneClick={() => {
          setSelectedNodeId(null);
          setDetailsNodeId(null);
          setActionError(null);
        }}
        defaultEdgeOptions={WORKFLOW_CANVAS_DEFAULT_EDGE_OPTIONS}
        nodesDraggable={false}
        nodesConnectable={false}
        edgesReconnectable={false}
        connectionMode={ConnectionMode.Strict}
        elementsSelectable
        deleteKeyCode={null}
        fitView
        colorMode={effectiveTheme}
        className={WORKFLOW_CANVAS_CLASS_NAMES.reactFlow}
        aria-label={t('workflow.runCanvas.canvasLabel')}
      >
        <Background
          variant={BackgroundVariant.Dots}
          size={1}
          color={WORKFLOW_CANVAS_COLOR_TOKENS.grid}
        />
        <Controls
          className={WORKFLOW_CANVAS_CLASS_NAMES.controls}
          fitViewOptions={{ duration: reducedMotion ? 0 : 240 }}
        />
      </ReactFlow>

      {attentionItem ? (
        <WorkflowRunAttentionCard
          item={attentionItem}
          itemCount={attentionItems.length}
          graph={graph}
          onOpenNode={() => openNodeDetails(attentionItem.nodeId)}
        />
      ) : null}

      <WorkflowRunNodeDetailsDialog
        open={Boolean(detailsNodeId && detailsGraphNode)}
        actionError={
          actionError?.nodeId === detailsNodeId ? actionError.message : null
        }
        gatePending={gatePending}
        gateSubmission={gateSubmission}
        graph={graph}
        isSelectingConditionBranch={conditionPending}
        navigation={navigation}
        node={detailsGraphNode}
        onApprove={() => void handleHumanGateDecision('approve')}
        onClose={() => {
          setDetailsNodeId(null);
          setActionError(null);
        }}
        onReject={() => void handleHumanGateDecision('reject')}
        onSelectConditionBranch={(targetNodeIds) =>
          void handleSelectConditionBranch(targetNodeIds)
        }
        projectId={projectId}
        run={run}
        execution={detailsExecution}
        work={detailsWork}
      />
    </div>
  );
}

function WorkflowRunAttentionCard({
  item,
  itemCount,
  graph,
  onOpenNode,
}: {
  item: WorkflowRuntimeAttentionItem;
  itemCount: number;
  graph: WorkflowGraph;
  onOpenNode: () => void;
}) {
  const { t } = useTranslation('common');
  const node = graph.nodes.find((candidate) => candidate.id === item.nodeId);
  const title =
    node?.data.display_name ||
    (node ? getWorkflowNodeKindLabel(node.type, t) : item.nodeId);

  return (
    <aside
      className="absolute right-6 top-6 z-20 w-[min(360px,calc(100%-3rem))] rounded-lg border border-secondary bg-panel p-base text-normal shadow-lg max-md:right-3 max-md:top-3 max-md:w-[calc(100%-1.5rem)]"
      aria-labelledby="workflow-runtime-attention-title"
      aria-live="polite"
    >
      <div className="flex items-start gap-3">
        {item.kind === 'failed' ? (
          <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-error" />
        ) : (
          <User className="mt-0.5 h-4 w-4 shrink-0 text-warning" />
        )}
        <div className="min-w-0 flex-1">
          <h2
            id="workflow-runtime-attention-title"
            className="text-sm font-semibold text-high"
          >
            {t('workflow.runCanvas.attentionTitle')}
          </h2>
          <p className="mt-half break-words text-xs text-normal">
            {t(
              item.kind === 'failed'
                ? 'workflow.runCanvas.attentionFailed'
                : 'workflow.runCanvas.attentionWaiting',
              { node: title }
            )}
          </p>
          {itemCount > 1 ? (
            <p className="mt-half text-xs text-low">
              {t('workflow.runCanvas.moreAttentionItems', {
                count: itemCount - 1,
              })}
            </p>
          ) : null}
          <Button
            type="button"
            size="xs"
            className="mt-base"
            onClick={onOpenNode}
          >
            {t('workflow.runCanvas.openNode')}
          </Button>
        </div>
      </div>
    </aside>
  );
}

interface WorkflowRunNodeDetailsDialogProps {
  open: boolean;
  actionError: string | null;
  gatePending: boolean;
  gateSubmission: GateSubmission | null;
  graph: WorkflowGraph;
  isSelectingConditionBranch: boolean;
  navigation: ReturnType<typeof useAppNavigation>;
  node: WorkflowNode | null;
  onApprove: () => void;
  onClose: () => void;
  onReject: () => void;
  onSelectConditionBranch: (targetNodeIds: string[]) => void;
  projectId: string;
  run: WorkflowRunResponse;
  execution: WorkflowNodeExecutionResponse | null;
  work: WorkflowNodeWorkView | null;
}

function WorkflowRunNodeDetailsDialog({
  open,
  actionError,
  gatePending,
  gateSubmission,
  graph,
  isSelectingConditionBranch,
  navigation,
  node,
  onApprove,
  onClose,
  onReject,
  onSelectConditionBranch,
  projectId,
  run,
  execution,
  work,
}: WorkflowRunNodeDetailsDialogProps) {
  const { t } = useTranslation('common');
  if (!node) return null;

  const actionGate = getWorkflowNodeActionGate(work);
  const taskTarget = getWorkflowNodeExecutionTarget(execution, work);
  const workspaceHref = buildRunWorkspaceHref(projectId, run);
  const sessionHref =
    taskTarget?.kind === 'agent-session'
      ? buildWorkspaceSessionHref(workspaceHref, taskTarget.sessionId)
      : null;
  const statusLabel = work
    ? t(`workflow.nodeStatus.${workflowNodeStatusKey(work.status)}`)
    : t('workflow.runCanvas.notExecutedYet');
  const title =
    node.data.display_name || getWorkflowNodeKindLabel(node.type, t);
  const isGateActionPending = gatePending && gateSubmission?.nodeId === node.id;
  const output = execution?.output_text?.trim();
  const error = execution?.error_text?.trim();
  const splitBatches = getWorkflowSplitBatchesForNode(run, node.id);
  const executionIds = new Set(
    run.nodes.filter((item) => item.node_id === node.id).map((item) => item.id)
  );
  const handoffs = run.handoffs.filter((handoff) =>
    executionIds.has(handoff.node_execution_id)
  );
  const reusedResults = work?.status === 'reused' ? work.reused_results : [];
  const waitingPrompt =
    work?.status === 'awaiting_human' || work?.status === 'awaiting_arena'
      ? output || t('workflow.runCanvas.reviewNodeToProceed')
      : null;

  return (
    <FloatingPanel
      open={open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) onClose();
      }}
      portal={false}
      autoFocus={false}
      restoreFocus
      closeLabel={t('workflow.runCanvas.closeNodeDetails')}
      className="absolute bottom-6 right-6 top-6 w-[min(440px,calc(100vw-3rem))] max-md:inset-0 max-md:w-full max-md:max-w-none max-md:rounded-none"
      contentClassName="flex min-h-0 flex-col overflow-hidden"
      data-testid="workflow-run-node-details"
    >
      <FloatingPanelHeader>
        <FloatingPanelTitle>{title}</FloatingPanelTitle>
        <FloatingPanelDescription>
          {getWorkflowNodeKindLabel(node.type, t)} · {statusLabel}
        </FloatingPanelDescription>
      </FloatingPanelHeader>

      <FloatingPanelBody className="min-h-0 flex-1 space-y-base overflow-y-auto">
        <section aria-labelledby="workflow-node-status-heading">
          <h3
            id="workflow-node-status-heading"
            className="text-xs font-semibold uppercase tracking-normal text-low"
          >
            {t('workflow.dashboard.status')}
          </h3>
          <div className="mt-half flex items-center gap-half text-sm text-high">
            {getRuntimeStatusIcon(work?.status)}
            <span>{statusLabel}</span>
          </div>
        </section>

        {splitBatches.map((batch) => (
          <WorkflowSplitBatchDetails
            key={batch.id}
            batch={batch}
            graph={graph}
            projectId={projectId}
            run={run}
            workspaceHref={workspaceHref}
          />
        ))}

        {handoffs.length > 0 ? (
          <section className="space-y-half">
            <h3 className="text-xs font-semibold text-high">
              {t('workflow.split.handoffFiles')}
            </h3>
            <WorkflowFileReferences
              runId={run.id}
              files={handoffs.flatMap((handoff) => [
                { path: handoff.handoff_path },
                ...handoff.files,
              ])}
            />
          </section>
        ) : null}

        {waitingPrompt ? (
          <section className="rounded border border-warning/40 bg-warning/10 p-base">
            <h3 className="text-xs font-semibold text-warning">
              {t('workflow.runCanvas.waitingInformation')}
            </h3>
            <p className="mt-half whitespace-pre-wrap text-xs text-high">
              {waitingPrompt}
            </p>
          </section>
        ) : null}

        {error ? (
          <section className="rounded border border-error/40 bg-error/10 p-base">
            <h3 className="text-xs font-semibold text-error">
              {t('workflow.dashboard.error')}
            </h3>
            <pre className="mt-half whitespace-pre-wrap break-words text-xs text-error">
              {error}
            </pre>
          </section>
        ) : null}

        {output && !waitingPrompt ? (
          <section>
            <h3 className="text-xs font-semibold uppercase tracking-normal text-low">
              {t('workflow.runCanvas.currentOutput')}
            </h3>
            <pre className="mt-half whitespace-pre-wrap break-words rounded border border-secondary bg-primary p-base text-xs text-high">
              {output}
            </pre>
          </section>
        ) : null}

        {work?.status === 'reused' ? (
          <section aria-labelledby="workflow-reused-results-heading">
            <h3
              id="workflow-reused-results-heading"
              className="text-xs font-semibold text-high"
            >
              {t('workflow.runCanvas.reusedResults')}
            </h3>
            <p className="mt-half text-xs text-low">
              {t('workflow.runCanvas.reusedResultsHint')}
            </p>
            <div className="mt-base space-y-base">
              {reusedResults.map((result) => (
                <article
                  key={`${result.source_run_id}:${result.source_node_execution_id}:${result.iteration}`}
                  className="space-y-half rounded border border-secondary bg-primary p-base"
                  data-testid="workflow-run-reused-result"
                >
                  <h4 className="text-xs font-medium text-high">
                    {t('workflow.runCanvas.reusedIteration', {
                      iteration: result.iteration,
                    })}
                  </h4>
                  <pre className="whitespace-pre-wrap break-words text-xs text-high">
                    {result.output_text ??
                      t('workflow.runCanvas.noSourceOutput')}
                  </pre>
                  <details className="rounded border border-secondary">
                    <summary className="cursor-pointer px-half py-half text-xs text-low focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand">
                      {t('workflow.runCanvas.reuseSource')}
                    </summary>
                    <div className="space-y-half border-t border-secondary p-half">
                      <MetadataRow label={t('workflow.runCanvas.sourceRunId')}>
                        {result.source_run_id}
                      </MetadataRow>
                      <MetadataRow
                        label={t('workflow.runCanvas.sourceExecutionId')}
                      >
                        {result.source_node_execution_id}
                      </MetadataRow>
                    </div>
                  </details>
                </article>
              ))}
            </div>
          </section>
        ) : !execution ? (
          <p className="text-xs text-low">
            {t('workflow.runCanvas.nodeNotExecuted')}
          </p>
        ) : !output && !error && !waitingPrompt ? (
          <p className="text-xs text-low">
            {t('workflow.runCanvas.noRuntimeOutput')}
          </p>
        ) : null}

        {node.type === 'condition' &&
        actionGate.canSelectConditionBranch &&
        execution ? (
          <ConditionRouterActionPanel
            graph={graph}
            conditionNode={node}
            isSelecting={isSelectingConditionBranch}
            onSelectBranch={onSelectConditionBranch}
            selectedExecution={execution}
          />
        ) : null}

        {actionError ? (
          <p className="text-xs text-error" role="alert">
            {actionError}
          </p>
        ) : null}

        {execution ? (
          <details className="rounded border border-secondary bg-primary">
            <summary className="cursor-pointer px-base py-half text-xs font-medium text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand">
              {t('workflow.runCanvas.technicalDetails')}
            </summary>
            <div className="space-y-half border-t border-secondary p-base">
              <MetadataRow label={t('workflow.runCanvas.executionId')}>
                {execution.id}
              </MetadataRow>
              <MetadataRow label={t('workflow.runCanvas.nodeId')}>
                {execution.node_id}
              </MetadataRow>
              <MetadataRow label={t('workflow.runCanvas.taskId')}>
                {execution.execution_id ?? t('workflow.dashboard.notAvailable')}
              </MetadataRow>
              <MetadataRow label={t('workflow.nodeSession.sessionId')}>
                {execution.session_id ?? t('workflow.dashboard.notAvailable')}
              </MetadataRow>
              <MetadataRow
                label={t('workflow.nodeSession.orchestrationNodeExecutionId')}
              >
                {execution.orchestration_node_execution_id ??
                  t('workflow.dashboard.notAvailable')}
              </MetadataRow>
              <MetadataRow label={t('workflow.nodeSession.agentRunId')}>
                {execution.agent_run_id ?? t('workflow.dashboard.notAvailable')}
              </MetadataRow>
              <MetadataRow label={t('workflow.runCanvas.started')}>
                {execution.started_at ?? t('workflow.dashboard.notStarted')}
              </MetadataRow>
              <MetadataRow label={t('workflow.runCanvas.finished')}>
                {execution.finished_at ?? t('workflow.runCanvas.notFinished')}
              </MetadataRow>
            </div>
          </details>
        ) : null}
      </FloatingPanelBody>

      {actionGate.canApprove ||
      actionGate.canReject ||
      sessionHref ||
      taskTarget?.kind === 'arena' ? (
        <FloatingPanelFooter>
          {actionGate.canReject ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={gatePending}
              aria-busy={isGateActionPending}
              onClick={onReject}
            >
              {isGateActionPending
                ? t('workflow.runCanvas.updatingDecision')
                : t('workflow.dashboard.reject')}
            </Button>
          ) : null}
          {actionGate.canApprove ? (
            <Button
              type="button"
              size="sm"
              disabled={gatePending}
              aria-busy={isGateActionPending}
              onClick={onApprove}
            >
              {isGateActionPending
                ? t('workflow.runCanvas.updatingDecision')
                : t('workflow.runCanvas.approveAndContinue')}
            </Button>
          ) : null}
          {sessionHref ? (
            <Button asChild size="sm">
              <a href={sessionHref}>
                {t('workflow.runCanvas.openFullSession')}
                <ExternalLink className="ml-half h-3.5 w-3.5" />
              </a>
            </Button>
          ) : null}
          {taskTarget?.kind === 'arena' ? (
            <Button
              type="button"
              size="sm"
              disabled={!navigation.goToProjectTaskArena}
              title={
                navigation.goToProjectTaskArena
                  ? undefined
                  : navigation.projectWorkflowUnavailableReason
              }
              onClick={() =>
                navigation.goToProjectTaskArena?.(
                  projectId,
                  run.task_id,
                  taskTarget.arenaGroupId
                )
              }
            >
              {t('workflow.runCanvas.viewArenaResults')}
              <ExternalLink className="ml-half h-3.5 w-3.5" />
            </Button>
          ) : null}
        </FloatingPanelFooter>
      ) : null}
    </FloatingPanel>
  );
}

function ConditionRouterActionPanel({
  graph,
  conditionNode,
  isSelecting,
  onSelectBranch,
  selectedExecution,
}: {
  graph: WorkflowGraph;
  conditionNode: WorkflowNode;
  isSelecting: boolean;
  onSelectBranch: (targetNodeIds: string[]) => void;
  selectedExecution: WorkflowNodeExecutionResponse;
}) {
  const { t } = useTranslation('common');
  const [selectedTargetIds, setSelectedTargetIds] = useState<string[]>([]);
  const routerOutput = parseConditionRouterOutput(
    selectedExecution.output_text
  );
  const question =
    getConditionRouterHumanPrompt(routerOutput) ??
    selectedExecution.output_text ??
    t('workflow.runCanvas.reviewNodeToProceed');
  const reason = getConditionRouterReason(routerOutput);
  const isMulti = conditionNode.data.routing_mode === 'multi';
  const branchOptions = useMemo(
    () => buildConditionBranchOptions(graph, conditionNode, t),
    [conditionNode, graph, t]
  );

  useEffect(() => {
    setSelectedTargetIds([]);
  }, [selectedExecution.id]);

  const toggleTarget = (targetNodeId: string) => {
    setSelectedTargetIds((current) =>
      current.includes(targetNodeId)
        ? current.filter((id) => id !== targetNodeId)
        : [...current, targetNodeId]
    );
  };

  return (
    <section className="space-y-base rounded border border-warning/40 bg-warning/10 p-base">
      <div>
        <h3 className="text-xs font-semibold text-warning">
          {t('workflow.runCanvas.humanActionRequired')}
        </h3>
        <p className="mt-half whitespace-pre-wrap text-xs text-high">
          {question}
        </p>
        {reason ? (
          <p className="mt-half whitespace-pre-wrap text-xs text-low">
            {reason}
          </p>
        ) : null}
      </div>
      <div className="space-y-half">
        {branchOptions.length === 0 ? (
          <p className="text-xs text-low">
            {t('workflow.runCanvas.noConditionBranches')}
          </p>
        ) : (
          branchOptions.map((option) => {
            const checked = selectedTargetIds.includes(option.targetNodeId);
            return (
              <div
                key={option.targetNodeId}
                className={cn(
                  'flex items-start gap-half rounded border bg-panel p-half text-xs',
                  checked ? 'border-brand/45' : 'border-secondary'
                )}
              >
                {isMulti ? (
                  <Checkbox
                    checked={checked}
                    disabled={isSelecting}
                    onCheckedChange={() => toggleTarget(option.targetNodeId)}
                    className="mt-0.5"
                  />
                ) : null}
                <div className="min-w-0 flex-1">
                  <div className="truncate font-semibold text-high">
                    {option.targetLabel}
                  </div>
                  {option.condition ? (
                    <div className="mt-1 line-clamp-2 text-low">
                      {option.condition}
                    </div>
                  ) : null}
                </div>
                {isMulti ? null : (
                  <Button
                    type="button"
                    size="xs"
                    disabled={isSelecting}
                    onClick={() => onSelectBranch([option.targetNodeId])}
                  >
                    {isSelecting
                      ? t('workflow.runCanvas.selectingConditionBranch')
                      : t('workflow.runCanvas.selectConditionBranch')}
                  </Button>
                )}
              </div>
            );
          })
        )}
      </div>
      {isMulti ? (
        <Button
          type="button"
          size="xs"
          disabled={isSelecting || selectedTargetIds.length === 0}
          onClick={() => onSelectBranch(selectedTargetIds)}
        >
          {isSelecting
            ? t('workflow.runCanvas.selectingConditionBranch')
            : t('workflow.runCanvas.continueSelectedBranches')}
        </Button>
      ) : null}
    </section>
  );
}

function getRuntimeStatusIcon(
  status: WorkflowNodeWorkStatus | undefined
): ReactNode {
  switch (status) {
    case 'running':
    case 'starting':
      return (
        <Activity className="h-4 w-4 text-brand motion-safe:animate-spin" />
      );
    case 'succeeded':
      return <CheckCircle className="h-4 w-4 text-success" />;
    case 'reused':
      return <History className="h-4 w-4 text-brand" />;
    case 'failed':
      return <AlertCircle className="h-4 w-4 text-error" />;
    case 'awaiting_human':
      return <User className="h-4 w-4 text-warning" />;
    case 'awaiting_arena':
      return <Swords className="h-4 w-4 text-warning" />;
    default:
      return <Clock className="h-4 w-4 text-low" />;
  }
}

function MetadataRow({
  children,
  label,
}: {
  children: ReactNode;
  label: string;
}) {
  return (
    <div className="rounded border border-secondary bg-panel p-half">
      <div className="text-xs font-semibold uppercase text-low">{label}</div>
      <div className="mt-1 break-all text-xs text-high">{children}</div>
    </div>
  );
}
