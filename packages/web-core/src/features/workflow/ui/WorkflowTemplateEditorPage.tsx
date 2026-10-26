import { useState, useEffect, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import {
  useWorkflowTemplate,
  useWorkflowTemplateMutations,
} from '@/shared/hooks/useWorkflowTemplates';
import {
  useWorkflowAttemptForWorkflow,
  useWorkflowAttemptMutations,
} from '@/shared/hooks/useWorkflowAttempts';
import { useWorkflowScheduledTask } from '@/shared/hooks/useScheduledTasks';
import { useProjectContext } from '@/shared/hooks/useProjectContext';
import {
  createWorkflowCanvasStageGroup,
  createWorkflowCanvasStickyNote,
  createDefaultWorkflowGraph,
  instantiateWorkflowGraphTemplate,
  migrateWorkflowGraph,
  tidyWorkflowGraph,
  type WorkflowGraph,
  type WorkflowNodeKind,
  type WorkflowNodePosition,
} from '../model/workflowGraph';
import {
  buildWorkflowRunInput,
  getWorkflowRunErrorMessage,
} from '../model/taskWorkflow';
import {
  acknowledgeLocalWorkflowSave,
  acknowledgeWorkflowPublication,
  acknowledgeWorkflowSave,
  commitWorkflowAuthoringGraph,
  createWorkflowAuthoringState,
  createWorkflowSaveSnapshot,
  dispatchWorkflowAuthoringCommand,
  redoWorkflowAuthoring,
  undoWorkflowAuthoring,
  validateWorkflowAuthoringGraph,
  type WorkflowAuthoringCommand,
  type WorkflowAuthoringState,
  type WorkflowCommandHistoryEntry,
} from '../model/workflowAuthoring';
import { isWorkflowNodeAuthorable } from '../model/workflowNodeCatalog';
import {
  deleteTaskWorkflowAttemptDraft,
  parseTaskWorkflowAttemptDraftRouteId,
  readTaskWorkflowAttemptDraft,
  saveTaskWorkflowAttemptDraft,
  type TaskWorkflowAttemptDraft,
} from '../model/workflowAttemptDraftStorage';
import { consumeWorkflowTemplateNodeFocus } from '../model/workflowTemplateNodeFocus';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { WorkflowRevisionConflictError } from '@/shared/lib/workflowApi';
import { WorkflowCanvas } from './WorkflowCanvas';
import { WorkflowNodeTypePicker } from './WorkflowNodeTypePicker';
import { WorkflowConfigurationFrame } from './WorkflowConfigurationFrame';
import { WorkflowEdgeInspector } from './WorkflowEdgeInspector';
import { WorkflowNodeInspector } from './WorkflowNodeInspector';
import { WorkflowRouterConfigPanel } from './WorkflowRouterConfigPanel';
import { WorkflowMainAgentDialog } from './WorkflowMainAgentDialog';
import { WorkflowMainSessionButton } from './WorkflowMainSessionButton';
import { ScheduledTaskDialog } from './ScheduledTaskDialog';
import { useWorkflowRepositorySelection } from './useWorkflowRepositorySelection';
import {
  getWorkflowAuthoringIssueMessage,
  getWorkflowDefaultGraphLabels,
  getWorkflowDefaultNodeData,
} from './workflowI18n';
import { validateWorkflowGraph } from './WorkflowValidationPanel';
import { Button } from '@vibe/ui/components/Button';
import {
  DegradedState,
  ErrorState,
  LoadingState,
} from '@vibe/ui/components/StateSurface';
import {
  Loader2,
  MoreHorizontal,
  ArrowLeft,
  Save,
  Copy,
  CheckCircle2,
  CalendarClock,
  LayoutGrid,
  Play as PlayIcon,
  GitBranch,
  StickyNote,
  Ungroup,
  Redo2,
  Undo2,
  Trash2,
} from 'lucide-react';
import { ReactFlowProvider } from '@xyflow/react';
import type { ExecutorConfig } from 'shared/types';
import {
  workflowDraftWorkspaceInput,
  type WorkflowWorkspaceInput,
} from '../model/workflowWorkspaceSelection';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@vibe/ui/components/Dropdown';
import { ConfirmDialog } from '@vibe/ui/components/ConfirmDialog';
import {
  Dialog as DescriptionDialog,
  DialogContent as DescriptionDialogContent,
  DialogTitle as DescriptionDialogTitle,
} from '@vibe/ui/components/Dialog';
import { WorkspaceContextHeader } from '@/shared/components/WorkspaceContextHeader';
import { AgentIcon, getAgentName } from '@/shared/components/AgentIcon';
import { useBlocker } from '@tanstack/react-router';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/KeyboardDialog';

const NEW_NODE_OFFSET_X = 340;
const NEW_NODE_OFFSET_Y = 0;
const NODE_COLLISION_X = 320;
const NODE_COLLISION_Y = 160;
interface NodeContextMenuState {
  nodeId: string;
  x: number;
  y: number;
}

interface PendingConnectionDrop {
  source: string;
  sourceHandle: string;
  position: WorkflowNodePosition;
  anchorPoint: { x: number; y: number };
}

interface DeletionToastState {
  count: number;
  undoDepth: number;
}

function isWorkflowTextInput(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tagName = target.tagName.toLowerCase();
  return (
    target.isContentEditable ||
    tagName === 'input' ||
    tagName === 'textarea' ||
    tagName === 'select'
  );
}

function avoidWorkflowNodeOverlap(
  graph: WorkflowGraph,
  position: WorkflowNodePosition
): WorkflowNodePosition {
  let next = { ...position };
  let guard = 0;

  while (
    guard < 24 &&
    graph.nodes.some((node) => {
      const existing = node.position;
      if (!existing) return false;
      return (
        Math.abs(existing.x - next.x) < NODE_COLLISION_X &&
        Math.abs(existing.y - next.y) < NODE_COLLISION_Y
      );
    })
  ) {
    next = {
      x: next.x + 40,
      y: next.y + 120,
    };
    guard += 1;
  }

  return next;
}

function getNewWorkflowNodePosition({
  graph,
  selectedNodeId,
  requestedPosition,
}: {
  graph: WorkflowGraph;
  selectedNodeId: string | null;
  requestedPosition?: WorkflowNodePosition;
}): WorkflowNodePosition {
  if (requestedPosition) {
    return avoidWorkflowNodeOverlap(graph, requestedPosition);
  }

  const selectedNode = graph.nodes.find((node) => node.id === selectedNodeId);
  if (selectedNode?.position) {
    return avoidWorkflowNodeOverlap(graph, {
      x: selectedNode.position.x + NEW_NODE_OFFSET_X,
      y: selectedNode.position.y + NEW_NODE_OFFSET_Y,
    });
  }

  return avoidWorkflowNodeOverlap(graph, { x: 400, y: 160 });
}

function getWorkflowNodeBounds(graph: WorkflowGraph) {
  const positions = graph.nodes
    .map((node) => node.position)
    .filter((position): position is WorkflowNodePosition => Boolean(position));

  if (positions.length === 0) {
    return {
      minX: 120,
      minY: 160,
      maxX: 780,
      maxY: 320,
    };
  }

  return positions.reduce(
    (bounds, position) => ({
      minX: Math.min(bounds.minX, position.x),
      minY: Math.min(bounds.minY, position.y),
      maxX: Math.max(bounds.maxX, position.x),
      maxY: Math.max(bounds.maxY, position.y),
    }),
    {
      minX: Number.POSITIVE_INFINITY,
      minY: Number.POSITIVE_INFINITY,
      maxX: Number.NEGATIVE_INFINITY,
      maxY: Number.NEGATIVE_INFINITY,
    }
  );
}

function getNewStickyNotePosition(graph: WorkflowGraph): WorkflowNodePosition {
  const bounds = getWorkflowNodeBounds(graph);
  return {
    x: bounds.minX,
    y: Math.max(20, bounds.minY - 170),
  };
}

function getNewStageGroupPosition(graph: WorkflowGraph): {
  position: WorkflowNodePosition;
  size: { width: number; height: number };
} {
  const bounds = getWorkflowNodeBounds(graph);
  return {
    position: {
      x: Math.max(40, bounds.minX - 50),
      y: Math.max(40, bounds.minY - 65),
    },
    size: {
      width: Math.max(520, bounds.maxX - bounds.minX + 360),
      height: Math.max(220, bounds.maxY - bounds.minY + 220),
    },
  };
}

export interface WorkflowTemplateEditorPageProps {
  projectId: string;
  workflowId: string;
}

export function WorkflowTemplateEditorPage({
  projectId,
  workflowId,
}: WorkflowTemplateEditorPageProps) {
  const { t } = useTranslation('common');
  const localDraftId = parseTaskWorkflowAttemptDraftRouteId(workflowId);
  const isLocalDraft = localDraftId !== null;
  const [storedLocalDraft, setLocalDraft] =
    useState<TaskWorkflowAttemptDraft | null>(() =>
      localDraftId ? readTaskWorkflowAttemptDraft(localDraftId) : null
    );
  const localDraft = localDraftId
    ? storedLocalDraft?.id === localDraftId
      ? storedLocalDraft
      : readTaskWorkflowAttemptDraft(localDraftId)
    : null;
  const {
    data: loadedTemplate,
    isLoading,
    isFetching,
    error,
    refetch: refetchTemplate,
  } = useWorkflowTemplate(workflowId, { enabled: !isLocalDraft });
  const {
    data: workflowAttempt,
    isLoading: isWorkflowAttemptLoading,
    error: workflowAttemptError,
  } = useWorkflowAttemptForWorkflow(workflowId, { enabled: !isLocalDraft });
  const { data: scheduledTask } = useWorkflowScheduledTask(
    projectId,
    workflowId,
    { enabled: !isLocalDraft }
  );
  const { updateTemplate, createTemplate, isUpdating, isCreating } =
    useWorkflowTemplateMutations();
  const { createAttempt, isCreatingAttempt, runAttempt, isRunningAttempt } =
    useWorkflowAttemptMutations();
  const navigation = useAppNavigation();
  const { getTask } = useProjectContext();

  const [authoringState, setAuthoringState] =
    useState<WorkflowAuthoringState | null>(null);
  const graph = authoringState?.graph ?? null;
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedNodeIds, setSelectedNodeIds] = useState<string[]>([]);
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(null);
  const [isRouterConfigPanelOpen, setIsRouterConfigPanelOpen] = useState(false);
  const [mainAgentDialogWorkflowId, setMainAgentDialogWorkflowId] = useState<
    string | null
  >(null);
  const [contextMenu, setContextMenu] = useState<NodeContextMenuState | null>(
    null
  );
  const [pendingConnectionDrop, setPendingConnectionDrop] =
    useState<PendingConnectionDrop | null>(null);
  const [deletionToast, setDeletionToast] = useState<DeletionToastState | null>(
    null
  );
  const [isSavingBeforeLeave, setIsSavingBeforeLeave] = useState(false);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [isDescriptionOpen, setIsDescriptionOpen] = useState(false);
  const moreActionsRef = useRef<HTMLButtonElement>(null);
  const [graphParseError, setGraphParseError] = useState<string | null>(null);
  const [runStartError, setRunStartError] = useState<string | null>(null);
  const [isStartingRun, setIsStartingRun] = useState(false);
  const [validationTouched, setValidationTouched] = useState(false);
  const consumedQueuedFocusForWorkflowRef = useRef<string | null>(null);
  const initializedTemplateIdRef = useRef<string | null>(null);
  const metadataBaselineRef = useRef({ name: '', description: '' });
  const template = useMemo(
    () =>
      isLocalDraft
        ? localDraft
          ? {
              id: workflowId,
              source: 'project' as const,
              project_id: projectId,
              name: localDraft.name,
              description: localDraft.taskDescription ?? null,
              graph_json: localDraft.graphJson,
              revision: 0,
              created_at: localDraft.createdAt,
              updated_at: localDraft.createdAt,
            }
          : undefined
        : loadedTemplate,
    [isLocalDraft, loadedTemplate, localDraft, projectId, workflowId]
  );
  const draftTask = localDraft ? getTask(localDraft.taskId) : null;
  const task = workflowAttempt ? getTask(workflowAttempt.task_id) : draftTask;
  const { selectWorkflowRepositories } = useWorkflowRepositorySelection({
    projectId,
    taskId: workflowAttempt?.task_id ?? localDraft?.taskId ?? '',
    taskTitle: task?.title ?? localDraft?.taskTitle ?? name,
  });
  const formatWorkflowRunError = (err: unknown) => {
    if (err instanceof WorkflowRevisionConflictError) {
      return t('workflow.errors.revisionConflict', {
        currentRevision: err.conflict.current_revision,
      });
    }

    return getWorkflowRunErrorMessage(err, {
      repositoryMessage: t('workflow.errors.repositoryRequired'),
      fallbackMessage: t('workflow.errors.startFailed'),
    });
  };
  const defaultGraphLabels = useMemo(
    () => getWorkflowDefaultGraphLabels(t),
    [t]
  );

  useEffect(() => {
    setLocalDraft(
      localDraftId ? readTaskWorkflowAttemptDraft(localDraftId) : null
    );
  }, [localDraftId]);

  const isSystem = template?.source === 'system';
  const definitionLocked = !!workflowAttempt?.definition_locked_at;
  const readOnly =
    isSystem ||
    definitionLocked ||
    (!isLocalDraft && (isWorkflowAttemptLoading || !!workflowAttemptError));
  const hasUnsavedChanges = Boolean(
    !readOnly &&
      (authoringState?.dirty ||
        name !== metadataBaselineRef.current.name ||
        description !== metadataBaselineRef.current.description)
  );
  const allowNavigationRef = useRef(false);
  const navigationBlocker = useBlocker({
    shouldBlockFn: () => hasUnsavedChanges && !allowNavigationRef.current,
    enableBeforeUnload: false,
    withResolver: true,
  });

  useEffect(() => {
    if (!hasUnsavedChanges) return;
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', handleBeforeUnload);
    return () => window.removeEventListener('beforeunload', handleBeforeUnload);
  }, [hasUnsavedChanges]);

  // Initialize graph from template
  useEffect(() => {
    if (template && initializedTemplateIdRef.current !== template.id) {
      initializedTemplateIdRef.current = template.id;
      setName(template.name || '');
      setDescription(template.description || '');
      metadataBaselineRef.current = {
        name: template.name || '',
        description: template.description || '',
      };
      try {
        const parsed = JSON.parse(template.graph_json) as WorkflowGraph;
        if (parsed && typeof parsed === 'object' && parsed.version) {
          setAuthoringState(
            createWorkflowAuthoringState(
              migrateWorkflowGraph(parsed),
              template.revision
            )
          );
          setGraphParseError(null);
        } else {
          setAuthoringState(
            createWorkflowAuthoringState(
              createDefaultWorkflowGraph(defaultGraphLabels),
              template.revision
            )
          );
          setGraphParseError(t('workflow.errors.invalidGraph'));
        }
      } catch {
        setAuthoringState(
          createWorkflowAuthoringState(
            createDefaultWorkflowGraph(defaultGraphLabels),
            template.revision
          )
        );
        setGraphParseError(t('workflow.errors.parseGraphFailed'));
      }
    }
  }, [defaultGraphLabels, t, template]);

  useEffect(() => {
    if (!graph || consumedQueuedFocusForWorkflowRef.current === workflowId) {
      return;
    }

    consumedQueuedFocusForWorkflowRef.current = workflowId;
    const queuedFocus = consumeWorkflowTemplateNodeFocus(workflowId);
    if (!queuedFocus) return;

    const focusedNode = graph.nodes.find(
      (node) => node.id === queuedFocus.nodeId
    );
    if (
      !focusedNode ||
      focusedNode.type === 'start' ||
      focusedNode.type === 'end'
    ) {
      return;
    }

    setSelectedNodeId(focusedNode.id);
    setSelectedNodeIds([focusedNode.id]);
    setSelectedEdgeId(null);
    setContextMenu(null);
  }, [graph, workflowId]);

  const persistWorkflowGraph = async (
    stateToSave: WorkflowAuthoringState = authoringState!
  ) => {
    if (readOnly) throw new Error(t('workflow.management.definitionLocked'));
    const snapshot = createWorkflowSaveSnapshot(stateToSave);
    const metadataSnapshot = { name, description };
    if (isLocalDraft && localDraft) {
      const nextDraft = {
        ...localDraft,
        name: metadataSnapshot.name,
        graphJson: JSON.stringify(snapshot.graph),
      };
      saveTaskWorkflowAttemptDraft(nextDraft);
      setLocalDraft(nextDraft);
      setAuthoringState((current) =>
        current ? acknowledgeLocalWorkflowSave(current, snapshot) : current
      );
      metadataBaselineRef.current = metadataSnapshot;
      return snapshot.graph;
    }

    const updatedTemplate = await updateTemplate({
      workflowId,
      payload: {
        expected_revision: snapshot.expectedRevision,
        name: metadataSnapshot.name,
        description: metadataSnapshot.description,
        graph_json: JSON.stringify(snapshot.graph),
      },
    });
    setAuthoringState((current) =>
      current
        ? acknowledgeWorkflowSave(current, snapshot, updatedTemplate.revision)
        : current
    );
    metadataBaselineRef.current = metadataSnapshot;
    return snapshot.graph;
  };

  const commitGraph = (
    nextGraph: WorkflowGraph,
    label: WorkflowCommandHistoryEntry['label'] = 'configure-node'
  ): WorkflowAuthoringState | null => {
    if (!authoringState || readOnly) return null;
    const nextState = commitWorkflowAuthoringGraph(
      authoringState,
      nextGraph,
      label
    );
    setAuthoringState(nextState);
    return nextState;
  };

  const dispatchAuthoringCommand = (command: WorkflowAuthoringCommand) => {
    if (!authoringState || readOnly) return null;
    setDeletionToast(null);
    const result = dispatchWorkflowAuthoringCommand(authoringState, command);
    if (result.issue) {
      setRunStartError(getWorkflowAuthoringIssueMessage(result.issue, t));
      return result;
    }
    setValidationTouched(false);
    setRunStartError(null);
    setAuthoringState(result.state);
    const removedNodeCount = Math.max(
      0,
      authoringState.graph.nodes.length - result.state.graph.nodes.length
    );
    const removedEdgeCount = Math.max(
      0,
      authoringState.graph.edges.length - result.state.graph.edges.length
    );
    if (removedNodeCount + removedEdgeCount > 0) {
      setDeletionToast({
        count: removedNodeCount || removedEdgeCount,
        undoDepth: result.state.undoStack.length,
      });
    }
    return result;
  };

  const handleUndo = () => {
    if (readOnly) return;
    setDeletionToast(null);
    setAuthoringState((current) =>
      current ? undoWorkflowAuthoring(current) : current
    );
    setSelectedNodeId(null);
    setSelectedNodeIds([]);
    setSelectedEdgeId(null);
    setContextMenu(null);
    closeRouterConfigPanel();
  };

  const handleRedo = () => {
    if (readOnly) return;
    setDeletionToast(null);
    setAuthoringState((current) =>
      current ? redoWorkflowAuthoring(current) : current
    );
    setSelectedNodeId(null);
    setSelectedNodeIds([]);
    setSelectedEdgeId(null);
    setContextMenu(null);
    closeRouterConfigPanel();
  };

  const createAttemptFromLocalDraft = async (
    nextGraph: WorkflowGraph,
    navigateAfterSave = true,
    configureMainAgentAfterSave = false
  ) => {
    if (!localDraft || !localDraftId) return null;

    let workspace = workflowDraftWorkspaceInput(localDraft);
    if (workspace.repos.length === 0 && !workspace.directory_path) {
      const selectedRepos = await selectWorkflowRepositories();
      if (!selectedRepos) {
        return null;
      }
      workspace = selectedRepos;
    }

    const attempt = await createAttempt({
      projectId,
      taskId: localDraft.taskId,
      payload: {
        name,
        graph_json: JSON.stringify(nextGraph),
        ...workspace,
      },
    });

    deleteTaskWorkflowAttemptDraft(localDraftId);
    if (navigateAfterSave) {
      if (configureMainAgentAfterSave) {
        setMainAgentDialogWorkflowId(attempt.workflow_id);
      }
      allowNavigationRef.current = true;
      navigation.goToProjectWorkflowEdit(projectId, attempt.workflow_id, {
        replace: true,
      });
      queueMicrotask(() => {
        allowNavigationRef.current = false;
      });
    }
    return attempt;
  };

  const handleSave = async (
    navigateAfterLocalSave = true,
    configureMainAgentAfterSave = false
  ): Promise<boolean> => {
    if (!graph || readOnly) return false;
    setRunStartError(null);
    if (
      validateWorkflowGraph(graph, { includeRunReadiness: false }).length > 0 ||
      validateWorkflowAuthoringGraph(graph).length > 0 ||
      graphParseError
    ) {
      setValidationTouched(true);
      return false;
    }
    try {
      if (isLocalDraft) {
        return Boolean(
          await createAttemptFromLocalDraft(
            graph,
            navigateAfterLocalSave,
            configureMainAgentAfterSave
          )
        );
      }
      if (authoringState) await persistWorkflowGraph(authoringState);
      return true;
    } catch (err) {
      setRunStartError(formatWorkflowRunError(err));
      return false;
    }
  };

  const closeRouterConfigPanel = () => {
    setIsRouterConfigPanelOpen(false);
  };

  const openRouterConfigPanel = () => {
    setIsRouterConfigPanelOpen(true);
    setSelectedEdgeId(null);
    setContextMenu(null);
  };

  const handleRouterConfigChange = (executorConfig: ExecutorConfig) => {
    if (!graph || !authoringState || readOnly) return;
    const nextGraph = {
      ...graph,
      router_executor_config: executorConfig,
    };
    setRunStartError(null);
    setAuthoringState(
      commitWorkflowAuthoringGraph(authoringState, nextGraph, 'configure-node')
    );
  };

  const handleStartRunFromGraph = async (nextGraph: WorkflowGraph) => {
    if (isLocalDraft) {
      setRunStartError(t('workflow.errors.saveBeforeRun'));
      return;
    }

    if (
      !workflowAttempt ||
      isSystem ||
      isWorkflowAttemptLoading ||
      workflowAttemptError
    ) {
      setRunStartError(t('workflow.errors.notLinkedToAttempt'));
      return;
    }

    const runValidationIssues = validateWorkflowGraph(nextGraph, {
      includeRunReadiness: true,
    });
    const runAuthoringIssues = validateWorkflowAuthoringGraph(nextGraph);
    if (
      runValidationIssues.length > 0 ||
      runAuthoringIssues.length > 0 ||
      graphParseError
    ) {
      setValidationTouched(true);
      return;
    }

    setIsStartingRun(true);
    setRunStartError(null);
    try {
      let workspace: WorkflowWorkspaceInput = { repos: [] };
      if (!workflowAttempt.workspace_id) {
        const selectedRepos = await selectWorkflowRepositories();
        if (!selectedRepos) {
          return;
        }
        workspace = selectedRepos;
      }

      if (!authoringState) return;
      if (!definitionLocked) await persistWorkflowGraph(authoringState);
      const run = await runAttempt({
        attemptId: workflowAttempt.id,
        payload: {
          workspace_id: null,
          trigger_source: 'manual',
          input_text: buildWorkflowRunInput({
            title: task?.title ?? name,
            description: task?.description ?? description,
          }),
          ...workspace,
        },
      });

      navigation.goToProjectWorkflowRun(projectId, run.id);
    } catch (err) {
      setRunStartError(formatWorkflowRunError(err));
    } finally {
      setIsStartingRun(false);
    }
  };

  const handleOpenLatestRun = () => {
    if (!workflowAttempt?.latest_run_id) return;
    navigation.goToProjectWorkflowRun(projectId, workflowAttempt.latest_run_id);
  };

  const handleOpenScheduledTask = () => {
    void ScheduledTaskDialog.show({
      projectId,
      workflowId,
      workflowName: name || t('workflow.templates.untitled'),
      existingTask: scheduledTask ?? null,
    });
  };

  const handleCopy = async () => {
    if (!graph) return;
    const result = await createTemplate({
      projectId,
      payload: {
        name: t('workflow.editor.copyName', {
          name: name || t('workflow.templates.newWorkflowName'),
        }),
        description,
        graph_json: JSON.stringify(instantiateWorkflowGraphTemplate(graph)),
      },
    });
    navigation.goToProjectWorkflowEdit(projectId, result.id);
  };

  const handleSaveAsTemplate = async () => {
    if (!graph || !isValid || isCreating) return;

    const result = await createTemplate({
      projectId,
      payload: {
        name: t('workflow.editor.savedTemplateName', {
          name: name || t('workflow.templates.newWorkflowName'),
          defaultValue: '{{name}} template',
        }),
        description,
        graph_json: JSON.stringify(instantiateWorkflowGraphTemplate(graph)),
      },
    });

    await ConfirmDialog.show({
      title: t('workflow.editor.templateSavedTitle', {
        defaultValue: 'Template saved',
      }),
      message: t('workflow.editor.templateSavedMessage', {
        name: result.name,
        defaultValue:
          '"{{name}}" is now available when creating workflow attempts.',
      }),
      confirmText: t('ok'),
      showCancelButton: false,
    });
  };

  const handleBack = () => {
    if (localDraft && !hasUnsavedChanges) {
      deleteTaskWorkflowAttemptDraft(localDraft.id);
    }
    if (localDraft) {
      navigation.goToProjectTask(projectId, localDraft.taskId);
      return;
    }
    if (workflowAttempt) {
      navigation.goToProjectTask(projectId, workflowAttempt.task_id);
      return;
    }
    navigation.goToProjectWorkflows(projectId);
  };

  const handleContinueEditing = () => {
    if (navigationBlocker.status === 'blocked') navigationBlocker.reset();
  };

  const handleDiscardAndLeave = () => {
    if (localDraft) deleteTaskWorkflowAttemptDraft(localDraft.id);
    allowNavigationRef.current = true;
    if (navigationBlocker.status === 'blocked') navigationBlocker.proceed();
  };

  const handleSaveAndLeave = async () => {
    if (navigationBlocker.status !== 'blocked') return;
    setIsSavingBeforeLeave(true);
    const saved = await handleSave(false);
    if (!saved) {
      setIsSavingBeforeLeave(false);
      return;
    }
    allowNavigationRef.current = true;
    navigationBlocker.proceed();
  };

  const handleUndoDeletion = () => {
    if (!deletionToast) return;
    setAuthoringState((current) => {
      if (!current || current.undoStack.length !== deletionToast.undoDepth) {
        return current;
      }
      return undoWorkflowAuthoring(current);
    });
    setDeletionToast(null);
  };

  const addWorkflowNode = ({
    kind,
    position,
  }: {
    kind: WorkflowNodeKind;
    position?: WorkflowNodePosition;
  }) => {
    if (!graph || readOnly) return;
    const newPosition = getNewWorkflowNodePosition({
      graph,
      selectedNodeId: null,
      requestedPosition: position,
    });
    if (!isWorkflowNodeAuthorable(kind)) return;
    const nodeId = `node-${crypto.randomUUID().slice(0, 8)}`;
    const result = dispatchAuthoringCommand({
      type: 'create-node',
      nodeType: kind,
      nodeId,
      position: newPosition,
      data: getWorkflowDefaultNodeData(kind, t),
    });
    if (!result || result.issue) return;
    setSelectedNodeId(nodeId);
    setSelectedNodeIds([nodeId]);
    setSelectedEdgeId(null);
    setIsRouterConfigPanelOpen(false);
    setContextMenu(null);
  };

  const handleAddNode = (
    kind: WorkflowNodeKind,
    position?: WorkflowNodePosition
  ) => {
    if (!isWorkflowNodeAuthorable(kind)) return;
    addWorkflowNode({ kind, position });
  };

  const handleOpenNodeEdit = (nodeId: string) => {
    const node = graph?.nodes.find((candidate) => candidate.id === nodeId);
    if (!node || node.type === 'start' || node.type === 'end') return;
    setSelectedNodeId(nodeId);
    setSelectedNodeIds([nodeId]);
    setSelectedEdgeId(null);
    closeRouterConfigPanel();
    setContextMenu(null);
  };

  const handleCanvasChange = (newGraph: WorkflowGraph) => {
    setValidationTouched(false);
    setRunStartError(null);
    commitGraph(newGraph, 'configure-node');
  };

  const handleNodesMove = (positions: Record<string, WorkflowNodePosition>) => {
    dispatchAuthoringCommand({ type: 'move-nodes', positions });
  };

  const handleConnectNodes = (connection: {
    source: string;
    sourceHandle: string;
    target: string;
  }) => {
    setPendingConnectionDrop(null);
    dispatchAuthoringCommand({ type: 'connect', ...connection });
  };

  const handleCreateConnectedNode = (
    kind: Exclude<WorkflowNodeKind, 'start' | 'end'>
  ) => {
    if (!graph || !pendingConnectionDrop || readOnly) return;
    const nodeId = `node-${crypto.randomUUID().slice(0, 8)}`;
    const result = dispatchAuthoringCommand({
      type: 'create-connected-node',
      source: pendingConnectionDrop.source,
      sourceHandle: pendingConnectionDrop.sourceHandle,
      nodeType: kind,
      nodeId,
      position: getNewWorkflowNodePosition({
        graph,
        selectedNodeId: null,
        requestedPosition: pendingConnectionDrop.position,
      }),
      data: getWorkflowDefaultNodeData(kind, t),
    });
    setPendingConnectionDrop(null);
    if (!result || result.issue) return;
    setSelectedNodeId(nodeId);
    setSelectedNodeIds([nodeId]);
    setSelectedEdgeId(null);
    closeRouterConfigPanel();
  };

  const handleReconnectEdge = (connection: {
    edgeId: string;
    source: string;
    sourceHandle: string;
    target: string;
  }) => {
    dispatchAuthoringCommand({ type: 'reconnect', ...connection });
  };

  const handleSplitEdgeWithNode = (input: {
    edgeId: string;
    nodeId: string;
    position: WorkflowNodePosition;
  }) => {
    const result = dispatchAuthoringCommand({
      type: 'split-edge-with-node',
      ...input,
    });
    if (!result || result.issue) return;
    setSelectedNodeId(input.nodeId);
    setSelectedNodeIds([input.nodeId]);
    setSelectedEdgeId(null);
  };

  const handleTidyGraph = () => {
    if (!graph || readOnly) return;
    commitGraph(tidyWorkflowGraph(graph), 'move-nodes');
  };

  const handleAddStickyNote = () => {
    if (!graph || readOnly) return;
    commitGraph(
      {
        ...graph,
        canvas: {
          ...graph.canvas,
          notes: [
            ...(graph.canvas?.notes ?? []),
            createWorkflowCanvasStickyNote({
              title: t('workflow.defaultGraph.noteTitle'),
              content: t('workflow.defaultGraph.noteContent'),
              position: getNewStickyNotePosition(graph),
            }),
          ],
        },
      },
      'configure-node'
    );
  };

  const handleAddStageGroup = () => {
    if (!graph || readOnly) return;
    const { position, size } = getNewStageGroupPosition(graph);
    commitGraph(
      {
        ...graph,
        canvas: {
          ...graph.canvas,
          groups: [
            ...(graph.canvas?.groups ?? []),
            createWorkflowCanvasStageGroup({
              title: t('workflow.defaultGraph.newStageTitle'),
              description: t('workflow.defaultGraph.newStageDescription'),
              position,
              size,
            }),
          ],
        },
      },
      'configure-node'
    );
  };

  const handleNodeChange = (
    nodeId: string,
    dataUpdates: Partial<WorkflowGraph['nodes'][number]['data']>
  ) => {
    if (!graph || readOnly) return;
    setRunStartError(null);
    dispatchAuthoringCommand({
      type: 'configure-node',
      nodeId,
      patch: dataUpdates,
    });
  };

  const handleDuplicateNode = (nodeId: string) => {
    if (!graph || readOnly) return;
    const node = graph.nodes.find((candidate) => candidate.id === nodeId);
    if (!node || node.type === 'start' || node.type === 'end') return;

    const duplicateId = `node-${crypto.randomUUID().slice(0, 8)}`;
    const result = dispatchAuthoringCommand({
      type: 'duplicate-node',
      nodeId,
      duplicateId,
      position: getNewWorkflowNodePosition({
        graph,
        selectedNodeId: null,
        requestedPosition: {
          x: (node.position?.x ?? 360) + 80,
          y: (node.position?.y ?? 160) + 80,
        },
      }),
      data: {
        display_name: `${String(
          node.data.display_name ??
            getWorkflowDefaultNodeData(node.type, t).display_name ??
            node.type
        )} ${t('workflow.editor.copySuffix')}`,
      },
    });
    if (!result || result.issue) return;
    setSelectedNodeId(duplicateId);
    setSelectedNodeIds([duplicateId]);
    setSelectedEdgeId(null);
    closeRouterConfigPanel();
    setContextMenu(null);
  };

  const handleDeleteNode = (nodeId: string) => {
    if (!graph || readOnly) return;
    const node = graph.nodes.find((candidate) => candidate.id === nodeId);
    if (!node || node.type === 'start' || node.type === 'end') return;

    const result = dispatchAuthoringCommand({
      type: 'delete-nodes',
      nodeIds: [nodeId],
    });
    if (!result || result.issue) return;
    if (selectedNodeId === nodeId) setSelectedNodeId(null);
    setSelectedNodeIds((current) =>
      current.filter((selectedId) => selectedId !== nodeId)
    );
    setSelectedEdgeId(null);
    closeRouterConfigPanel();
    setContextMenu(null);
  };

  const handleDeleteNodes = (nodeIds: string[]) => {
    if (!graph || readOnly || nodeIds.length === 0) return;
    const result = dispatchAuthoringCommand({ type: 'delete-nodes', nodeIds });
    if (!result || result.issue) return;
    setSelectedNodeId(null);
    setSelectedNodeIds([]);
    setSelectedEdgeId(null);
    closeRouterConfigPanel();
    setContextMenu(null);
  };

  const handleCloseConfiguration = () => {
    const nodeIdToRestore = selectedNodeId;
    closeRouterConfigPanel();
    setSelectedNodeId(null);
    setSelectedNodeIds([]);
    setSelectedEdgeId(null);
    window.requestAnimationFrame(() => {
      const canvas = document.querySelector<HTMLElement>(
        '[data-workflow-canvas-root="true"]'
      );
      const selectedNode = Array.from(
        canvas?.querySelectorAll<HTMLElement>('.react-flow__node[data-id]') ??
          []
      ).find((element) => element.dataset.id === nodeIdToRestore);
      (selectedNode ?? canvas)?.focus({ preventScroll: true });
    });
  };

  const handleDeleteEdge = (edgeId: string) => {
    if (!graph || readOnly) return;
    const result = dispatchAuthoringCommand({ type: 'delete-edge', edgeId });
    if (!result || result.issue) return;
    setSelectedEdgeId(null);
  };

  useEffect(() => {
    if (readOnly) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (isWorkflowTextInput(event.target) || event.altKey) return;
      const modifier = event.metaKey || event.ctrlKey;
      if (!modifier) return;
      const key = event.key.toLowerCase();
      if (key === 'z') {
        event.preventDefault();
        if (event.shiftKey) handleRedo();
        else handleUndo();
        return;
      }
      if (key === 'y' && !event.shiftKey) {
        event.preventDefault();
        handleRedo();
        return;
      }
      if (key === 'd' && !event.shiftKey && selectedNodeId && !selectedEdgeId) {
        event.preventDefault();
        handleDuplicateNode(selectedNodeId);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  });

  const selectedNode = useMemo(
    () => graph?.nodes.find((n) => n.id === selectedNodeId) ?? null,
    [graph, selectedNodeId]
  );

  const selectedEdge = useMemo(
    () => graph?.edges.find((edge) => edge.id === selectedEdgeId) ?? null,
    [graph, selectedEdgeId]
  );

  const contextMenuNode = useMemo(
    () => graph?.nodes.find((node) => node.id === contextMenu?.nodeId) ?? null,
    [contextMenu?.nodeId, graph]
  );

  const isInitializingTemplate = Boolean(
    template && (!graph || initializedTemplateIdRef.current !== template.id)
  );

  if (isLoading || isInitializingTemplate) {
    return (
      <LoadingState
        className="h-full w-full bg-primary"
        title={t('workflow.runCanvas.loadingGraph')}
      />
    );
  }

  if (!template || !graph) {
    const message =
      error instanceof Error
        ? error.message
        : error
          ? String(error)
          : t('workflow.errors.draftMissing');
    return (
      <ErrorState
        className="h-full w-full bg-primary"
        title={t('workflow.editor.loadFailed', { message })}
        action={
          !isLocalDraft ? (
            <Button
              type="button"
              variant="outline"
              loading={isFetching}
              onClick={() => void refetchTemplate()}
            >
              {t('buttons.retry')}
            </Button>
          ) : undefined
        }
      />
    );
  }

  const validationIssues = validateWorkflowGraph(graph, {
    includeRunReadiness: true,
  });
  const draftValidationIssues = validateWorkflowGraph(graph, {
    includeRunReadiness: false,
  });
  const authoringIssues = validateWorkflowAuthoringGraph(graph);
  const canvasValidationIssues = [
    ...validationIssues.map((issue) => ({
      ...issue,
      message: issue.splitScopeCode
        ? t(`workflow.split.scopeErrors.${issue.splitScopeCode}`)
        : issue.message,
    })),
    ...authoringIssues
      .filter((task) => task.code !== 'splitter-scope')
      .map((task) => ({
        type: 'error' as const,
        nodeId: task.nodeId,
        message: getWorkflowAuthoringIssueMessage(task, t),
      })),
  ];
  const isValid =
    draftValidationIssues.length === 0 &&
    authoringIssues.length === 0 &&
    !graphParseError;
  const isRunReady =
    validationIssues.length === 0 &&
    authoringIssues.length === 0 &&
    !graphParseError;
  const canRunWorkflowAttempt =
    !!workflowAttempt &&
    !isWorkflowAttemptLoading &&
    !isStartingRun &&
    !isRunningAttempt &&
    !isUpdating &&
    !isLocalDraft &&
    isRunReady &&
    !isSystem &&
    !workflowAttemptError;
  const configurableNode =
    selectedNode?.type === 'start' || selectedNode?.type === 'end'
      ? null
      : selectedNode;
  const edgeSource = selectedEdge
    ? graph.nodes.find((node) => node.id === selectedEdge.source)
    : null;
  const edgeTarget = selectedEdge
    ? graph.nodes.find((node) => node.id === selectedEdge.target)
    : null;
  const configurationObjectKey = isRouterConfigPanelOpen
    ? 'router-config'
    : selectedEdge
      ? `edge-${selectedEdge.id}`
      : `node-${configurableNode?.id ?? 'none'}`;
  const configurationTitle = isRouterConfigPanelOpen
    ? t('workflow.inspector.routerAgent', { defaultValue: 'Router agent' })
    : selectedEdge
      ? `${String(edgeSource?.data.display_name ?? selectedEdge.source)} → ${String(
          edgeTarget?.data.display_name ?? selectedEdge.target
        )}`
      : String(
          configurableNode?.data.display_name ??
            t('workflow.inspector.step', { defaultValue: 'Node' })
        );
  const configurationDescription = isRouterConfigPanelOpen
    ? t('workflow.inspector.routerSharedNote', {
        defaultValue: 'Used by all Condition nodes in this workflow.',
      })
    : selectedEdge
      ? t('workflow.inspector.edgeProperties')
      : configurableNode
        ? `${configurableNode.type} · ${t('workflow.inspector.step', {
            defaultValue: 'Node',
          })}`
        : '';

  return (
    <div className="workflow-canvas-shell flex h-full flex-col bg-primary">
      {mainAgentDialogWorkflowId === workflowId &&
      loadedTemplate &&
      !readOnly ? (
        <WorkflowMainAgentDialog
          key={loadedTemplate.id}
          projectId={projectId}
          template={loadedTemplate}
          taskId={workflowAttempt?.task_id}
          mainSessionWorkflowId={
            workflowAttempt?.template_id ?? workflowAttempt?.workflow_id
          }
          onPublished={(saved, previousRevision) => {
            setAuthoringState((current) =>
              current
                ? acknowledgeWorkflowPublication(
                    current,
                    previousRevision,
                    saved.revision
                  )
                : current
            );
          }}
          onClose={() => setMainAgentDialogWorkflowId(null)}
        />
      ) : null}
      <Dialog
        open={navigationBlocker.status === 'blocked'}
        onOpenChange={(open) => {
          if (!open && !isSavingBeforeLeave) handleContinueEditing();
        }}
        uncloseable={isSavingBeforeLeave}
      >
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>
              {t('workflow.editor.unsavedChangesTitle', {
                defaultValue: 'Unsaved workflow changes',
              })}
            </DialogTitle>
            <DialogDescription>
              {t('workflow.editor.unsavedChangesDescription', {
                defaultValue:
                  'Save the current draft before leaving, discard it, or continue editing.',
              })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="sm:justify-between">
            <Button
              type="button"
              variant="outline"
              disabled={isSavingBeforeLeave}
              onClick={handleContinueEditing}
            >
              {t('workflow.editor.continueEditing', {
                defaultValue: 'Continue editing',
              })}
            </Button>
            <div className="flex flex-col-reverse gap-2 sm:flex-row">
              <Button
                type="button"
                variant="outline"
                disabled={isSavingBeforeLeave}
                onClick={handleDiscardAndLeave}
              >
                {t('workflow.editor.discardAndLeave', {
                  defaultValue: 'Discard and leave',
                })}
              </Button>
              <Button
                type="button"
                disabled={isSavingBeforeLeave}
                onClick={() => void handleSaveAndLeave()}
              >
                {isSavingBeforeLeave ? (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                ) : null}
                {t('workflow.editor.saveAndLeave', {
                  defaultValue: 'Save and leave',
                })}
              </Button>
            </div>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <DescriptionDialog
        open={isDescriptionOpen}
        onOpenChange={setIsDescriptionOpen}
      >
        <DescriptionDialogContent
          className="max-w-md p-4"
          aria-describedby={undefined}
          closeLabel={t('buttons.close')}
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            moreActionsRef.current?.focus();
          }}
        >
          <DescriptionDialogTitle className="mb-3 pr-8">
            {t('workflow.editor.descriptionPlaceholder')}
          </DescriptionDialogTitle>
          <input
            value={description}
            onChange={(event) => setDescription(event.target.value)}
            disabled={readOnly}
            aria-label={t('workflow.editor.descriptionPlaceholder')}
            className="w-full rounded border border-secondary bg-primary px-3 py-2 text-sm text-normal outline-none focus-visible:ring-1 focus-visible:ring-brand"
          />
        </DescriptionDialogContent>
      </DescriptionDialog>

      {/* Toolbar */}
      <div className="workflow-editor-toolbar flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-secondary bg-primary px-4 py-2 sm:flex-nowrap">
        <div className="workflow-editor-toolbar__identity flex min-w-0 basis-full items-center gap-3 sm:basis-auto sm:flex-1">
          <Button
            variant="ghost"
            onClick={handleBack}
            className="flex h-9 w-9 items-center justify-center p-0 transition-colors hover:bg-secondary/20"
            aria-label={t('workflow.editor.backToWorkflows')}
          >
            <ArrowLeft className="h-4 w-4" />
          </Button>
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              readOnly={readOnly}
              className="min-w-0 w-full bg-transparent text-sm font-semibold text-high outline-none transition-colors hover:text-brand focus:text-brand"
              placeholder={t('workflow.editor.workflowNamePlaceholder')}
              aria-label={t('workflow.editor.workflowNamePlaceholder')}
            />
            {workflowAttempt || localDraft ? (
              <WorkspaceContextHeader
                workspaceId={workflowAttempt?.workspace_id}
                draftRepo={localDraft?.repos[0]}
                draftDirectoryPath={localDraft?.directoryPath}
                className="max-w-[620px]"
              />
            ) : null}
            {isSystem && (
              <span className="text-xs text-brand">
                {t('workflow.editor.systemTemplateReadOnly')}
              </span>
            )}
          </div>
        </div>
        <div className="workflow-editor-toolbar__actions flex max-w-full shrink-0 flex-wrap items-center gap-1.5 sm:flex-nowrap">
          <Button
            variant="outline"
            disabled={readOnly || !authoringState?.undoStack.length}
            onClick={handleUndo}
            className="flex h-9 w-9 items-center justify-center p-0"
            aria-label={t('workflow.editor.undo', {
              defaultValue: 'Undo',
            })}
            title={t('workflow.editor.undoShortcut', {
              defaultValue: 'Undo (Ctrl/Cmd + Z)',
            })}
          >
            <Undo2 className="h-4 w-4" />
          </Button>
          <Button
            variant="outline"
            disabled={readOnly || !authoringState?.redoStack.length}
            onClick={handleRedo}
            className="flex h-9 w-9 items-center justify-center p-0"
            aria-label={t('workflow.editor.redo', {
              defaultValue: 'Redo',
            })}
            title={t('workflow.editor.redoShortcut', {
              defaultValue: 'Redo (Ctrl/Cmd + Shift + Z)',
            })}
          >
            <Redo2 className="h-4 w-4" />
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                ref={moreActionsRef}
                variant="ghost"
                className="h-9 w-9 p-0"
                aria-label={t('workflow.editor.moreActions', {
                  defaultValue: 'More actions',
                })}
                title={t('workflow.editor.moreActions', {
                  defaultValue: 'More actions',
                })}
              >
                <MoreHorizontal className="h-4 w-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="end"
              className="w-64"
              onCloseAutoFocus={(event) => {
                if (isDescriptionOpen) event.preventDefault();
              }}
            >
              <DropdownMenuItem onSelect={() => setIsDescriptionOpen(true)}>
                {t('workflow.editor.descriptionPlaceholder')}
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem disabled={readOnly} onSelect={handleTidyGraph}>
                <LayoutGrid className="mr-2 h-4 w-4" />
                {t('workflow.editor.tidy')}
              </DropdownMenuItem>
              <DropdownMenuItem
                disabled={readOnly}
                onSelect={handleAddStickyNote}
              >
                <StickyNote className="mr-2 h-4 w-4" />
                {t('workflow.editor.note')}
              </DropdownMenuItem>
              <DropdownMenuItem
                disabled={readOnly}
                onSelect={handleAddStageGroup}
              >
                <Ungroup className="mr-2 h-4 w-4" />
                {t('workflow.editor.stage')}
              </DropdownMenuItem>
              <DropdownMenuItem
                disabled={readOnly}
                onSelect={openRouterConfigPanel}
              >
                <GitBranch className="mr-2 h-4 w-4" />
                {t('workflow.router.toolbar', { defaultValue: 'Router' })}
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={() => setValidationTouched(true)}>
                <CheckCircle2 className="mr-2 h-4 w-4" />
                {t('workflow.editor.validate')}
              </DropdownMenuItem>
              <DropdownMenuItem
                disabled={isLocalDraft}
                onSelect={handleOpenScheduledTask}
                title={
                  isLocalDraft
                    ? t('workflow.schedule.saveBeforeSchedule')
                    : undefined
                }
              >
                <CalendarClock className="mr-2 h-4 w-4" />
                {t('workflow.schedule.button')}
              </DropdownMenuItem>
              {workflowAttempt?.latest_run_id ? (
                <DropdownMenuItem onSelect={handleOpenLatestRun}>
                  {t('workflow.editor.openLatestRun')}
                </DropdownMenuItem>
              ) : null}
              {!isSystem ? (
                <DropdownMenuItem
                  onSelect={() => void handleSaveAsTemplate()}
                  disabled={isCreating || !isValid}
                >
                  <Copy className="mr-2 h-4 w-4" />
                  {t('workflow.editor.saveAsTemplate', {
                    defaultValue: 'Save as template',
                  })}
                </DropdownMenuItem>
              ) : null}
            </DropdownMenuContent>
          </DropdownMenu>
          {workflowAttempt ? (
            <WorkflowMainSessionButton
              key={workflowAttempt.id}
              projectId={projectId}
              workflowId={
                workflowAttempt.template_id ?? workflowAttempt.workflow_id
              }
              taskId={workflowAttempt.task_id}
              disabled={
                (!!workflowAttempt.main_session_bound_at &&
                  !workflowAttempt.main_session_id) ||
                (!loadedTemplate?.main_agent_config &&
                  !workflowAttempt.main_session_id &&
                  !workflowAttempt.main_session_bound_at)
              }
            />
          ) : null}
          <Button
            variant="outline"
            disabled={!canRunWorkflowAttempt}
            onClick={() => void handleStartRunFromGraph(graph)}
            className="flex items-center gap-2"
            aria-label={t('workflow.editor.runAttempt')}
            title={
              workflowAttempt
                ? undefined
                : isLocalDraft
                  ? t('workflow.errors.saveBeforeRun')
                  : t('workflow.errors.notLinkedToAttempt')
            }
          >
            <span className="inline-flex items-center gap-2">
              {isStartingRun || isRunningAttempt || isWorkflowAttemptLoading ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <PlayIcon className="h-4 w-4" />
              )}
              {t('workflow.editor.runWorkflow')}
            </span>
          </Button>
          {isSystem ? (
            <Button
              onClick={handleCopy}
              disabled={isCreating}
              className="flex items-center gap-2"
            >
              <span className="inline-flex items-center gap-2">
                {isCreating ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <Copy className="h-4 w-4" />
                )}
                {t('workflow.editor.copyToProject')}
              </span>
            </Button>
          ) : (
            <>
              <Button
                onClick={() => void handleSave()}
                disabled={
                  readOnly || isUpdating || isCreatingAttempt || !isValid
                }
                className="flex items-center gap-2"
              >
                <span className="inline-flex items-center gap-2">
                  {isUpdating || isCreatingAttempt ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    <Save className="h-4 w-4" />
                  )}
                  {t('buttons.save')}
                </span>
              </Button>
            </>
          )}
        </div>
      </div>

      <div className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-secondary bg-primary px-4 py-2 text-xs">
        <span className="text-low">{t('workflow.management.mainAgent')}</span>
        <span className="inline-flex items-center gap-1.5 text-normal">
          {loadedTemplate?.main_agent_config ? (
            <>
              <AgentIcon
                agent={loadedTemplate.main_agent_config.executor}
                className="h-4 w-4"
              />
              {getAgentName(loadedTemplate.main_agent_config.executor)}
            </>
          ) : (
            t('workflow.management.mainAgentNotConfigured')
          )}
        </span>
        {!isSystem && !definitionLocked ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={
              readOnly ||
              isUpdating ||
              isCreatingAttempt ||
              (isLocalDraft && !isValid)
            }
            loading={isCreatingAttempt}
            aria-label={t('workflow.management.configureMainAgent')}
            onClick={() => {
              if (isLocalDraft) void handleSave(true, true);
              else setMainAgentDialogWorkflowId(workflowId);
            }}
          >
            {isLocalDraft
              ? t('workflow.management.saveAndConfigure')
              : t('workflow.management.configureMainAgent')}
          </Button>
        ) : null}
        {!loadedTemplate?.main_agent_config &&
        !workflowAttempt?.main_session_id &&
        !workflowAttempt?.main_session_bound_at ? (
          <span className="text-low" role="status">
            {definitionLocked
              ? t('workflow.management.mainAgentMissingLocked')
              : t('workflow.management.mainAgentOptional')}
          </span>
        ) : null}
      </div>

      {definitionLocked ? (
        <p className="shrink-0 border-b border-secondary px-base py-half text-base text-low">
          {t('workflow.management.definitionLocked')}
        </p>
      ) : workflowAttemptError ? (
        <p
          role="alert"
          className="shrink-0 px-base py-half text-base text-error"
        >
          {t('workflow.management.instanceFailed')}
        </p>
      ) : null}

      {graphParseError ? (
        <div className="border-b border-error/30 bg-error/10 px-base py-half text-xs text-error">
          {graphParseError}
        </div>
      ) : runStartError ? (
        <div
          className="border-b border-error/30 bg-error/10 px-base py-half text-xs text-error"
          role="alert"
        >
          {runStartError}
        </div>
      ) : validationTouched && validationIssues.length > 0 ? (
        <div className="border-b border-brand/30 bg-brand/10 px-base py-half text-xs text-brand">
          {t('workflow.editor.validationFound', {
            count: validationIssues.length,
          })}
        </div>
      ) : null}

      {contextMenu &&
      contextMenuNode &&
      contextMenuNode.type !== 'start' &&
      contextMenuNode.type !== 'end' ? (
        <DropdownMenu
          open
          onOpenChange={(open) => {
            if (!open) setContextMenu(null);
          }}
        >
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label={t('workflow.editor.nodeActions', {
                defaultValue: 'Node actions',
              })}
              style={{
                position: 'fixed',
                left: contextMenu.x,
                top: contextMenu.y,
                width: 1,
                height: 1,
                padding: 0,
                border: 0,
                background: 'transparent',
                zIndex: 10000,
              }}
            />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" side="bottom" sideOffset={2}>
            <DropdownMenuItem
              onClick={() => handleOpenNodeEdit(contextMenu.nodeId)}
            >
              {t('buttons.edit')}
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={readOnly}
              onClick={() => handleDuplicateNode(contextMenu.nodeId)}
            >
              {t('workflow.editor.duplicate')}
            </DropdownMenuItem>
            <DropdownMenuItem disabled>
              {t('workflow.editor.runStep')}
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              variant="destructive"
              disabled={readOnly}
              onClick={() => void handleDeleteNode(contextMenu.nodeId)}
            >
              {t('buttons.delete')}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      ) : null}

      <div className="relative min-h-0 flex-1 overflow-hidden">
        {error ? (
          <DegradedState
            compact
            className="absolute left-1/2 top-4 z-20 w-[min(28rem,calc(100%-2rem))] -translate-x-1/2 border border-warning/30 shadow-md"
            title={t('workflow.editor.loadFailed', {
              message: error instanceof Error ? error.message : String(error),
            })}
            action={
              <Button
                type="button"
                variant="outline"
                size="sm"
                loading={isFetching}
                onClick={() => void refetchTemplate()}
              >
                {t('buttons.retry')}
              </Button>
            }
          />
        ) : null}

        <div className="absolute left-4 top-4 z-10">
          <WorkflowNodeTypePicker
            disabled={readOnly}
            onSelect={(kind) => handleAddNode(kind)}
          />
        </div>
        <ReactFlowProvider>
          <WorkflowCanvas
            graph={graph}
            validationIssues={canvasValidationIssues}
            selectedNodeId={selectedNodeId}
            selectedNodeIds={selectedNodeIds}
            selectedEdgeId={selectedEdgeId}
            readOnly={readOnly}
            onChange={handleCanvasChange}
            onNodesMove={handleNodesMove}
            onConnectNodes={handleConnectNodes}
            onConnectDrop={setPendingConnectionDrop}
            onReconnectEdge={handleReconnectEdge}
            onSplitEdgeWithNode={handleSplitEdgeWithNode}
            onEdgeDelete={handleDeleteEdge}
            onSelectionChange={(selection) => {
              setSelectedNodeId(selection.nodeId);
              setSelectedNodeIds(selection.nodeIds);
              setSelectedEdgeId(selection.edgeId);
              closeRouterConfigPanel();
              setContextMenu(null);
            }}
            onNodeDrop={handleAddNode}
            onNodeEdit={handleOpenNodeEdit}
            onNodeDelete={handleDeleteNode}
            onNodesDelete={handleDeleteNodes}
            onNodeContextMenu={(event) => {
              const node = graph.nodes.find(
                (candidate) => candidate.id === event.nodeId
              );
              closeRouterConfigPanel();
              setContextMenu(
                node && node.type !== 'start' && node.type !== 'end'
                  ? event
                  : null
              );
            }}
          />
        </ReactFlowProvider>

        {pendingConnectionDrop ? (
          <WorkflowNodeTypePicker
            open
            anchorPoint={pendingConnectionDrop.anchorPoint}
            disabled={readOnly}
            onOpenChange={(open) => {
              if (!open) setPendingConnectionDrop(null);
            }}
            onSelect={handleCreateConnectedNode}
          />
        ) : null}

        {selectedNodeIds.length > 1 ? (
          <div
            role="toolbar"
            aria-label={t('workflow.editor.multiSelectionActions', {
              defaultValue: 'Selected Node actions',
            })}
            className="absolute left-1/2 top-4 z-20 flex -translate-x-1/2 items-center gap-3 rounded-md border border-secondary bg-panel/95 px-3 py-2 text-sm text-high shadow-md backdrop-blur"
          >
            <span>
              {t('workflow.editor.nodesSelected', {
                count: selectedNodeIds.length,
                defaultValue: '{{count}} Nodes selected',
              })}
            </span>
            <Button
              type="button"
              variant="outline"
              className="h-8 gap-2 text-error"
              disabled={readOnly}
              onClick={() => handleDeleteNodes(selectedNodeIds)}
            >
              <Trash2 className="h-3.5 w-3.5" />
              {t('buttons.delete')}
            </Button>
          </div>
        ) : null}

        {validationTouched && canvasValidationIssues.length > 0 ? (
          <div
            role="status"
            className="absolute bottom-4 left-4 z-10 max-w-sm rounded-md border border-brand/30 bg-panel/95 px-3 py-2 text-xs text-brand shadow-md"
          >
            {t('workflow.editor.validationFound', {
              count: canvasValidationIssues.length,
            })}
          </div>
        ) : null}

        {deletionToast ? (
          <div
            role="status"
            aria-live="polite"
            className="absolute bottom-4 right-4 z-30 flex items-center gap-4 rounded-md border border-secondary bg-panel px-4 py-3 text-sm text-high shadow-lg"
          >
            <span>
              {t('workflow.editor.deletedObjects', {
                count: deletionToast.count,
                defaultValue: 'Deleted {{count}} item',
              })}
            </span>
            <Button
              type="button"
              variant="outline"
              className="h-8"
              onClick={handleUndoDeletion}
            >
              {t('workflow.editor.undo', { defaultValue: 'Undo' })}
            </Button>
          </div>
        ) : null}

        <WorkflowConfigurationFrame
          open={Boolean(
            isRouterConfigPanelOpen || selectedEdge || configurableNode
          )}
          title={configurationTitle}
          description={configurationDescription}
          objectKey={configurationObjectKey}
          onClose={handleCloseConfiguration}
        >
          {isRouterConfigPanelOpen ? (
            <WorkflowRouterConfigPanel
              routerExecutorConfig={graph.router_executor_config}
              readOnly={readOnly}
              error={runStartError}
              onChange={handleRouterConfigChange}
            />
          ) : selectedEdge ? (
            <WorkflowEdgeInspector
              edge={selectedEdge}
              nodes={graph.nodes}
              readOnly={readOnly}
              onDelete={handleDeleteEdge}
              onOpenSourceNode={(nodeId) => {
                setSelectedEdgeId(null);
                setSelectedNodeId(nodeId);
              }}
            />
          ) : (
            <WorkflowNodeInspector
              node={configurableNode}
              graph={graph}
              routerExecutorConfig={graph.router_executor_config}
              readOnly={readOnly}
              onChange={handleNodeChange}
              onConfigureRouter={openRouterConfigPanel}
              onDelete={handleDeleteNode}
            />
          )}
        </WorkflowConfigurationFrame>
      </div>
    </div>
  );
}
