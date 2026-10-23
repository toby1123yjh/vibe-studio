import { useCallback, useMemo, useState } from 'react';
import type { ExecutionSummary, WorkflowTemplateResponse } from 'shared/types';
import type { Task } from 'shared/remote-types';
import { CreateArenaDialog } from '@/features/arena';
import { useCreateTaskWorkflowAttempt } from '@/features/workflow';
import { shouldShowWorkflowTemplate } from '@/features/workflow/model/workflowTemplateVisibility';
import { WorkflowTemplatePickerDialog } from '@/features/workflow/ui/WorkflowTemplatePickerDialog';
import { TaskCommentsSectionContainer } from '@/pages/kanban/TaskCommentsSectionContainer';
import { TaskRelationshipsSectionContainer } from '@/pages/kanban/TaskRelationshipsSectionContainer';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useCurrentKanbanRouteState } from '@/shared/hooks/useCurrentKanbanRouteState';
import { useProjectContext } from '@/shared/hooks/useProjectContext';
import { useProjectWorkspaceCreateDraft } from '@/shared/hooks/useProjectWorkspaceCreateDraft';
import { useUserContext } from '@/shared/hooks/useUserContext';
import { useWorkflowTemplates } from '@/shared/hooks/useWorkflowTemplates';
import { useWorkspaceContext } from '@/shared/hooks/useWorkspaceContext';
import { getWorkspaceDefaults } from '@/shared/lib/workspaceDefaults';
import {
  buildLinkedTaskCreateState,
  buildLocalWorkspaceIdSet,
  buildWorkspaceCreateInitialState,
  buildWorkspaceCreatePrompt,
} from '@/shared/lib/workspaceCreateState';
import { TaskFloatingPanel } from './TaskFloatingPanel';
import type { ExecutionDeletionActions } from './ExecutionDeleteButton';

interface TaskFloatingPanelContainerProps extends ExecutionDeletionActions {
  task: Task;
  executions: ExecutionSummary[];
  onClose(): void;
  onOpenExecution(execution: ExecutionSummary): void;
  getExecutionUnavailableReason(execution: ExecutionSummary): string | null;
  agentUnavailableReason: string | null;
  workflowUnavailableReason: string | null;
  arenaUnavailableReason: string | null;
}

export function TaskFloatingPanelContainer({
  task,
  executions,
  onClose,
  onOpenExecution,
  onDeleteExecution,
  deletingSessionId,
  getExecutionUnavailableReason,
  agentUnavailableReason,
  workflowUnavailableReason,
  arenaUnavailableReason,
}: TaskFloatingPanelContainerProps) {
  const appNavigation = useAppNavigation();
  const routeState = useCurrentKanbanRouteState();
  const { workspaces } = useUserContext();
  const { activeWorkspaces, archivedWorkspaces } = useWorkspaceContext();
  const { openWorkspaceCreateFromState } = useProjectWorkspaceCreateDraft();
  const {
    projectId,
    statuses,
    tags,
    taskTags,
    updateTask,
    insertTaskTag,
    removeTaskTag,
  } = useProjectContext();
  const [busyAction, setBusyAction] = useState<
    'agent' | 'workflow' | 'arena' | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const { data: workflowTemplateData } = useWorkflowTemplates(projectId, {
    enabled: !!projectId,
  });
  const { createWorkflowAttempt, workflowCreateError } =
    useCreateTaskWorkflowAttempt({
      taskId: task.id,
      taskTitle: task.title,
      taskDescription: task.description,
    });

  const selectedTagIds = useMemo(
    () =>
      new Set(
        taskTags
          .filter((link) => link.task_id === task.id)
          .map((link) => link.tag_id)
      ),
    [task.id, taskTags]
  );
  const workflowTemplates = useMemo(
    () =>
      (workflowTemplateData?.workflows ?? []).filter(
        shouldShowWorkflowTemplate
      ),
    [workflowTemplateData]
  );

  const createAgentExecution = useCallback(async () => {
    setBusyAction('agent');
    setError(null);
    try {
      const localWorkspaceIds = buildLocalWorkspaceIdSet(
        activeWorkspaces,
        archivedWorkspaces
      );
      const defaults = await getWorkspaceDefaults(
        workspaces,
        localWorkspaceIds,
        projectId,
        routeState.hostId
      );
      const createState = buildWorkspaceCreateInitialState({
        prompt: null,
        defaults,
        linkedTask: buildLinkedTaskCreateState(task, projectId),
      });
      const draftId = await openWorkspaceCreateFromState(createState, {
        taskId: task.id,
      });
      if (!draftId) {
        throw new Error('Failed to prepare the agent workspace.');
      }
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : 'Failed to prepare the agent workspace.'
      );
    } finally {
      setBusyAction(null);
    }
  }, [
    activeWorkspaces,
    archivedWorkspaces,
    task,
    openWorkspaceCreateFromState,
    projectId,
    routeState.hostId,
    workspaces,
  ]);

  const createWorkflowExecution = useCallback(async () => {
    setBusyAction('workflow');
    setError(null);
    try {
      let template: WorkflowTemplateResponse | null =
        workflowTemplates[0] ?? null;
      if (workflowTemplates.length > 1) {
        const result = await WorkflowTemplatePickerDialog.show({
          templates: workflowTemplates.map((candidate) => ({
            id: candidate.id,
            name: candidate.name,
            description: candidate.description,
          })),
        });
        if (result.kind === 'canceled') return;
        template =
          workflowTemplates.find(
            (candidate) => candidate.id === result.templateId
          ) ?? null;
      }
      await createWorkflowAttempt({ template });
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : 'Failed to prepare the workflow.'
      );
    } finally {
      setBusyAction(null);
    }
  }, [createWorkflowAttempt, workflowTemplates]);

  const createArenaExecution = useCallback(async () => {
    setBusyAction('arena');
    setError(null);
    try {
      const result = await CreateArenaDialog.show({
        projectId,
        taskId: task.id,
        hostId: routeState.hostId,
        initialPrompt:
          buildWorkspaceCreatePrompt(task.title, task.description) ?? undefined,
      });
      if (result.kind === 'created') {
        appNavigation.goToProjectTaskArena?.(
          projectId,
          task.id,
          result.groupId
        );
      }
    } catch (cause) {
      setError(
        cause instanceof Error ? cause.message : 'Failed to start the Arena.'
      );
    } finally {
      setBusyAction(null);
    }
  }, [appNavigation, task, projectId, routeState.hostId]);

  const toggleTag = useCallback(
    (tagId: string) => {
      setError(null);
      const existing = taskTags.find(
        (link) => link.task_id === task.id && link.tag_id === tagId
      );
      const mutation = existing
        ? removeTaskTag(existing.id)
        : insertTaskTag({ task_id: task.id, tag_id: tagId });
      void mutation.persisted.catch((cause) => {
        setError(
          cause instanceof Error ? cause.message : 'Failed to update the tag.'
        );
      });
    },
    [insertTaskTag, task.id, taskTags, removeTaskTag]
  );
  const updateDescription = useCallback(
    (description: string | null) => {
      setError(null);
      void updateTask(task.id, { description }).persisted.catch((cause) => {
        setError(
          cause instanceof Error
            ? cause.message
            : 'Failed to update the description.'
        );
      });
    },
    [task.id, updateTask]
  );
  const updateStatus = useCallback(
    (statusId: string) => {
      setError(null);
      void updateTask(task.id, { status_id: statusId }).persisted.catch(
        (cause) => {
          setError(
            cause instanceof Error
              ? cause.message
              : 'Failed to update the status.'
          );
        }
      );
    },
    [task.id, updateTask]
  );

  return (
    <TaskFloatingPanel
      task={task}
      statuses={statuses.filter((status) => !status.hidden)}
      tags={tags}
      selectedTagIds={selectedTagIds}
      executions={executions}
      error={error ?? workflowCreateError}
      busyAction={busyAction}
      agentUnavailableReason={agentUnavailableReason}
      workflowUnavailableReason={workflowUnavailableReason}
      arenaUnavailableReason={arenaUnavailableReason}
      relationships={<TaskRelationshipsSectionContainer taskId={task.id} />}
      comments={<TaskCommentsSectionContainer taskId={task.id} />}
      onClose={onClose}
      onOpenExecution={onOpenExecution}
      onDeleteExecution={onDeleteExecution}
      deletingSessionId={deletingSessionId}
      getExecutionUnavailableReason={getExecutionUnavailableReason}
      onCreateAgent={() => void createAgentExecution()}
      onCreateWorkflow={() => void createWorkflowExecution()}
      onCreateArena={() => void createArenaExecution()}
      onUpdateDescription={updateDescription}
      onUpdateStatus={updateStatus}
      onToggleTag={toggleTag}
    />
  );
}
