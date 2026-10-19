import { useCallback, useEffect, useMemo, useRef } from 'react';
import { useInfiniteQuery } from '@tanstack/react-query';
import { useNavigate, useSearch } from '@tanstack/react-router';
import type { ExecutionCursor, ExecutionSummary } from 'shared/types';
import { mergeStableCursorItems } from '@/features/app-shell/model/appShell';
import { ProjectRightSidebarContainer } from '@/pages/kanban/ProjectRightSidebarContainer';
import { Actions } from '@/shared/actions';
import { useActions } from '@/shared/hooks/useActions';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useCurrentKanbanRouteState } from '@/shared/hooks/useCurrentKanbanRouteState';
import { useProjectContext } from '@/shared/hooks/useProjectContext';
import { executionDataApi } from '@/shared/lib/executionDataApi';
import { getHostRequestScopeQueryKey } from '@/shared/lib/hostRequestScope';
import { useDeleteExecutionSession } from '@/shared/hooks/useDeleteExecutionSession';
import { bulkUpdateTasks } from '@/shared/lib/remoteApi';
import {
  buildKanbanTaskComposerKey,
  openKanbanTaskComposer,
  useKanbanTaskComposer,
} from '@/shared/stores/useKanbanTaskComposerStore';
import {
  buildKanbanColumns,
  groupTopLevelExecutionsByTask,
  type KanbanMoveUpdate,
} from '../model/project-kanban';
import { TaskFloatingPanelContainer } from './TaskFloatingPanelContainer';
import { ProjectKanbanView } from './ProjectKanbanView';
import { ProjectBoardActions } from './ProjectBoardActions';

interface ProjectKanbanContainerProps {
  projectName: string;
  projectSource?: {
    title: string;
    description?: string;
    retry(): void;
    retrying?: boolean;
  };
}

const REMOTE_ARENA_UNAVAILABLE_REASON =
  'Arena comparison is unavailable in this deployment.';

type SharedSearchNavigate = (options: {
  search(previous: Record<string, unknown>): Record<string, unknown>;
  replace?: boolean;
}) => Promise<void>;

export function ProjectKanbanContainer({
  projectName,
  projectSource,
}: ProjectKanbanContainerProps) {
  const navigateSearch = useNavigate() as unknown as SharedSearchNavigate;
  const search = useSearch({ strict: false }) as {
    q?: string;
    session_id?: string;
  };
  const appNavigation = useAppNavigation();
  const { executeAction } = useActions();
  const routeState = useCurrentKanbanRouteState();
  const { projectId, tasks, statuses, tags, taskTags, getTask } =
    useProjectContext();
  const taskTriggerRef = useRef<HTMLElement | null>(null);
  const requestedExecutionCursorRef = useRef<string | null>(null);
  const composerKey = useMemo(
    () => buildKanbanTaskComposerKey(routeState.hostId, projectId),
    [projectId, routeState.hostId]
  );
  const taskComposer = useKanbanTaskComposer(composerKey);

  const executionsQuery = useInfiniteQuery({
    queryKey: [
      'project-executions',
      projectId,
      getHostRequestScopeQueryKey(routeState.hostId),
    ],
    queryFn: ({ pageParam, signal }) =>
      executionDataApi.listExecutions({
        projectId,
        cursor: pageParam,
        limit: 100,
        hostId: routeState.hostId,
        signal,
      }),
    initialPageParam: null as ExecutionCursor | null,
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    staleTime: 10_000,
  });
  const {
    data: executionPages,
    fetchNextPage,
    isError: isExecutionSourceError,
    isFetchNextPageError,
    isFetching: isExecutionSourceFetching,
    isFetchingNextPage,
    isPending: isExecutionSourcePending,
    refetch: refetchExecutions,
  } = executionsQuery;

  useEffect(() => {
    requestedExecutionCursorRef.current = null;
  }, [projectId, routeState.hostId]);

  useEffect(() => {
    const nextCursor = executionPages?.pages.at(-1)?.next_cursor;
    if (!nextCursor || isFetchingNextPage) return;

    const cursorKey = `${nextCursor.updated_at}:${nextCursor.id}`;
    if (requestedExecutionCursorRef.current === cursorKey) return;

    requestedExecutionCursorRef.current = cursorKey;
    void fetchNextPage();
  }, [fetchNextPage, isFetchingNextPage, executionPages?.pages]);

  const executions = useMemo(
    () =>
      (executionPages?.pages ?? []).reduce(
        (items, page) => mergeStableCursorItems(items, page.executions),
        [] as ExecutionSummary[]
      ),
    [executionPages?.pages]
  );
  const executionsByTask = useMemo(
    () => groupTopLevelExecutionsByTask(executions),
    [executions]
  );
  const columns = useMemo(
    () =>
      buildKanbanColumns({
        statuses,
        tasks,
        tags,
        taskTags,
        executions,
        query: search.q ?? '',
      }),
    [taskTags, tasks, search.q, statuses, tags, executions]
  );
  const selectedTask = routeState.taskId
    ? getTask(routeState.taskId)
    : undefined;
  const showCanonicalTaskPanel =
    selectedTask !== undefined &&
    !routeState.workspaceId &&
    !routeState.isWorkspaceCreateMode &&
    taskComposer === null;
  const showLegacyDeepPanel =
    taskComposer !== null ||
    routeState.workspaceId !== null ||
    routeState.isWorkspaceCreateMode;

  useEffect(() => {
    if (
      routeState.taskId &&
      !selectedTask &&
      !routeState.isWorkspaceCreateMode
    ) {
      appNavigation.goToProject(projectId, { replace: true });
    }
  }, [
    appNavigation,
    projectId,
    routeState.isWorkspaceCreateMode,
    routeState.taskId,
    selectedTask,
  ]);

  const updateSearch = useCallback(
    (q: string) => {
      void navigateSearch({
        search: (previous) => ({
          ...previous,
          q: q.trim() ? q : undefined,
        }),
        replace: true,
      });
    },
    [navigateSearch]
  );

  const openTask = useCallback(
    (taskId: string, trigger: HTMLElement) => {
      taskTriggerRef.current = trigger;
      appNavigation.goToProjectTask(projectId, taskId);
    },
    [appNavigation, projectId]
  );

  const closePanel = useCallback(() => {
    const trigger = taskTriggerRef.current;
    const taskId = routeState.taskId;
    appNavigation.goToProject(projectId, { replace: true });
    requestAnimationFrame(() => {
      const fallback = taskId
        ? document.querySelector<HTMLElement>(`[data-task-id="${taskId}"]`)
        : null;
      (trigger?.isConnected ? trigger : fallback)?.focus({
        preventScroll: true,
      });
    });
  }, [appNavigation, projectId, routeState.taskId]);

  const openExecution = useCallback(
    (execution: ExecutionSummary) => {
      const target = execution.open_target;
      switch (target.kind) {
        case 'agent':
          appNavigation.goToProjectTaskWorkspace(
            projectId,
            execution.task_id,
            target.workspace_id
          );
          return;
        case 'workflow':
          if (target.latest_run_id) {
            appNavigation.goToProjectWorkflowRun(
              projectId,
              target.latest_run_id
            );
          } else {
            appNavigation.goToProjectWorkflowEdit(
              projectId,
              target.workflow_id
            );
          }
          return;
        case 'arena': {
          appNavigation.goToProjectTaskArena?.(
            projectId,
            execution.task_id,
            target.arena_group_id
          );
        }
      }
    },
    [appNavigation, projectId]
  );
  const getExecutionUnavailableReason = useCallback(
    (execution: ExecutionSummary) => {
      switch (execution.open_target.kind) {
        case 'agent':
          return appNavigation.agentExecutionUnavailableReason ?? null;
        case 'workflow':
          return appNavigation.projectWorkflowUnavailableReason ?? null;
        case 'arena':
          return appNavigation.goToProjectTaskArena
            ? null
            : REMOTE_ARENA_UNAVAILABLE_REASON;
      }
    },
    [
      appNavigation.agentExecutionUnavailableReason,
      appNavigation.goToProjectTaskArena,
      appNavigation.projectWorkflowUnavailableReason,
    ]
  );

  const retryExecutionSource = useCallback(() => {
    if (isFetchNextPageError) {
      requestedExecutionCursorRef.current = null;
      void fetchNextPage();
      return;
    }
    void refetchExecutions();
  }, [fetchNextPage, isFetchNextPageError, refetchExecutions]);

  const executionSource = useMemo(() => {
    if (isExecutionSourcePending) {
      return {
        state: 'loading' as const,
        title: 'Loading executions…',
      };
    }
    if (isExecutionSourceError && executions.length === 0) {
      return {
        state: 'degraded' as const,
        title: 'Executions are unavailable.',
        description: 'Task data is still shown.',
        retry: retryExecutionSource,
        retrying: isExecutionSourceFetching,
      };
    }
    if (isFetchNextPageError) {
      return {
        state: 'degraded' as const,
        title: 'Some executions could not be loaded.',
        retry: retryExecutionSource,
        retrying: isExecutionSourceFetching,
      };
    }
    if (isExecutionSourceError) {
      return {
        state: 'degraded' as const,
        title: 'Executions could not be refreshed.',
        description: 'Previously loaded tasks remain available.',
        retry: retryExecutionSource,
        retrying: isExecutionSourceFetching,
      };
    }
    if (isFetchingNextPage) {
      return {
        state: 'loading' as const,
        title: 'Loading remaining execution tasks…',
      };
    }
    return { state: 'ready' as const };
  }, [
    retryExecutionSource,
    executions.length,
    isFetchNextPageError,
    isFetchingNextPage,
    isExecutionSourceFetching,
    isExecutionSourceError,
    isExecutionSourcePending,
  ]);

  const moveTasks = useCallback(
    async (updates: KanbanMoveUpdate[]) => {
      await bulkUpdateTasks(
        updates.map((update) => ({
          id: update.id,
          changes: {
            status_id: update.statusId,
            sort_order: update.sortOrder,
          },
        })),
        routeState.hostId
      );
    },
    [routeState.hostId]
  );
  const deleteTask = useCallback(
    async (taskId: string) => {
      await executeAction(Actions.DeleteTask, undefined, projectId, [taskId]);
    },
    [executeAction, projectId]
  );

  const { deleteSession, pendingSessionId: deletingSessionId } =
    useDeleteExecutionSession({
      hostId: routeState.hostId,
      scopeKey: JSON.stringify([routeState, projectId, search.session_id]),
    });
  const deleteExecution = useCallback(
    (execution: ExecutionSummary) => {
      if (execution.open_target.kind !== 'agent') return;
      void deleteSession({
        executionId: execution.id,
        sessionId: execution.open_target.session_id,
        workspaceId: execution.open_target.workspace_id,
        title: execution.title,
      });
    },
    [deleteSession]
  );

  const panel =
    showCanonicalTaskPanel && selectedTask ? (
      <aside className="vk-task-floating-panel" aria-label="Task details">
        <TaskFloatingPanelContainer
          key={selectedTask.id}
          task={selectedTask}
          executions={executionsByTask.get(selectedTask.id) ?? []}
          onClose={closePanel}
          onOpenExecution={openExecution}
          onDeleteExecution={deleteExecution}
          deletingSessionId={deletingSessionId}
          getExecutionUnavailableReason={getExecutionUnavailableReason}
          agentUnavailableReason={
            appNavigation.agentExecutionUnavailableReason ?? null
          }
          workflowUnavailableReason={
            appNavigation.projectWorkflowUnavailableReason ?? null
          }
          arenaUnavailableReason={
            appNavigation.goToProjectTaskArena
              ? null
              : REMOTE_ARENA_UNAVAILABLE_REASON
          }
        />
      </aside>
    ) : showLegacyDeepPanel ? (
      <aside
        className="vk-task-floating-panel"
        data-create-panel={
          taskComposer !== null || routeState.isWorkspaceCreateMode || undefined
        }
        aria-label="Task activity"
      >
        <ProjectRightSidebarContainer />
      </aside>
    ) : null;

  return (
    <ProjectKanbanView
      projectName={projectName}
      projectActions={
        <ProjectBoardActions
          projectId={projectId}
          projectName={projectName}
          hostId={routeState.hostId}
        />
      }
      columns={columns}
      taskCount={columns.reduce(
        (count, column) => count + column.tasks.length,
        0
      )}
      query={search.q ?? ''}
      selectedTaskId={routeState.taskId}
      dragDisabled={Boolean(search.q?.trim())}
      projectSource={projectSource}
      executionSource={executionSource}
      panel={panel}
      onQueryChange={updateSearch}
      onCreateTask={(statusId) =>
        openKanbanTaskComposer(composerKey, { statusId })
      }
      onOpenTask={openTask}
      onOpenExecution={openExecution}
      onDeleteExecution={deleteExecution}
      deletingSessionId={deletingSessionId}
      onDeleteTask={deleteTask}
      getExecutionUnavailableReason={getExecutionUnavailableReason}
      onMove={moveTasks}
    />
  );
}
