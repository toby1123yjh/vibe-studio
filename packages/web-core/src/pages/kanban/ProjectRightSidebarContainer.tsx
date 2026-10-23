import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { useTranslation } from 'react-i18next';
import { ArrowsOutIcon, XIcon } from '@phosphor-icons/react';
import { useProjectContext } from '@/shared/hooks/useProjectContext';
import { useUserContext } from '@/shared/hooks/useUserContext';
import {
  findAppShellRecentSession,
  useAppShellRecentSessions,
} from '@/shared/hooks/useAppShellRecentSessions';
import { ExecutionProcessesProvider } from '@/shared/providers/ExecutionProcessesProvider';
import { CreateModeProvider } from '@/features/create-mode/model/CreateModeProvider';
import { useWorkspaceSessions } from '@/shared/hooks/useWorkspaceSessions';
import { useWorkspaceRecord } from '@/shared/hooks/useWorkspaceRecord';
import { useWorkspaceRepo } from '@/shared/hooks/useWorkspaceRepo';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { WorkspaceConversationContent } from '@/features/workspace-chat/ui/WorkspaceConversationContent';
import { deriveAgentWorkbenchHeader } from '@/features/agent-workbench/model/agentWorkbenchHeader';
import { deriveWorkspaceDetailState } from '@/shared/lib/workspaceDetailState';
import { buildWorkspaceContext } from '@/shared/lib/workspaceContext';
import { CreateChatBoxContainer } from '@/shared/components/CreateChatBoxContainer';
import { KanbanTaskPanelContainer } from './KanbanTaskPanelContainer';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useCurrentKanbanRouteState } from '@/shared/hooks/useCurrentKanbanRouteState';
import {
  buildKanbanTaskComposerKey,
  closeKanbanTaskComposer,
  openKanbanTaskComposer,
  useKanbanTaskComposer,
} from '@/shared/stores/useKanbanTaskComposerStore';

interface WorkspaceSessionPanelProps {
  workspaceId: string;
  onClose: () => void;
}

interface WorkspaceCreatePanelProps {
  linkedTaskId: string | null;
  linkedTaskSimpleId: string | null;
  onOpenTask: (taskId: string) => void;
  onClose: () => void;
  children: ReactNode;
}

type TaskPanelResolution = 'resolving' | 'ready' | 'missing';

type RightPanelState =
  | { kind: 'closed' }
  | { kind: 'create-issue' }
  | { kind: 'task'; taskId: string; resolution: TaskPanelResolution }
  | { kind: 'task-workspace'; workspaceId: string }
  | { kind: 'workspace-create'; draftId: string; taskId: string | null };

function resolveTaskPanelResolution({
  taskId,
  hasTask,
  isProjectLoading,
  expectedTaskId,
}: {
  taskId: string;
  hasTask: boolean;
  isProjectLoading: boolean;
  expectedTaskId: string | null;
}): TaskPanelResolution {
  if (isProjectLoading) {
    return 'resolving';
  }

  if (hasTask) {
    return 'ready';
  }

  if (expectedTaskId === taskId) {
    return 'resolving';
  }

  return 'missing';
}

function WorkspaceCreatePanel({
  linkedTaskId,
  linkedTaskSimpleId,
  onOpenTask,
  onClose,
  children,
}: WorkspaceCreatePanelProps) {
  const { t } = useTranslation('tasks');
  const breadcrumbButtonClass =
    'min-w-0 text-sm text-normal truncate rounded-sm px-1 py-0.5 hover:bg-panel hover:text-high transition-colors';

  const handleOpenTask = useCallback(() => {
    if (linkedTaskId) {
      onOpenTask(linkedTaskId);
      return;
    }
    onClose();
  }, [linkedTaskId, onOpenTask, onClose]);

  return (
    <div className="relative flex h-full flex-1 flex-col bg-primary">
      <div className="flex items-center justify-between px-base py-half border-b shrink-0">
        <div className="flex items-center gap-half min-w-0 font-ibm-plex-mono">
          <button
            type="button"
            onClick={handleOpenTask}
            className={`${breadcrumbButtonClass} shrink-0`}
            aria-label="Open linked task"
          >
            {linkedTaskSimpleId ?? 'Task'}
          </button>
          <span className="text-low text-sm shrink-0">/</span>
          <span className={breadcrumbButtonClass}>
            {t('createWorkspaceFromPr.createWorkspace')}
          </span>
        </div>
        <div className="flex items-center gap-half">
          <button
            type="button"
            onClick={onClose}
            className="p-half rounded-sm text-low hover:text-normal hover:bg-panel transition-colors"
            aria-label="Close create workspace view"
          >
            <XIcon className="size-icon-sm" weight="bold" />
          </button>
        </div>
      </div>
      <div className="flex-1 min-h-0">{children}</div>
    </div>
  );
}

function WorkspaceSessionPanel({
  workspaceId,
  onClose,
}: WorkspaceSessionPanelProps) {
  const { t } = useTranslation('common');
  const hostId = useHostId();
  const appNavigation = useAppNavigation();
  const { projectId, getTask } = useProjectContext();
  const routeState = useCurrentKanbanRouteState();
  const { workspaces: remoteWorkspaces } = useUserContext();
  const recentSessions = useAppShellRecentSessions();
  const workspaceQuery = useWorkspaceRecord(workspaceId, {
    enabled: !!workspaceId,
  });
  const workspace = workspaceQuery.data;
  const reposQuery = useWorkspaceRepo(workspaceId);
  const {
    sessions,
    selectedSession,
    selectedSessionId,
    selectSession,
    isLoading: isSessionsLoading,
    error: sessionsError,
    retry: retrySessions,
    isNewSessionMode,
    startNewSession,
  } = useWorkspaceSessions(workspaceId, { enabled: !!workspaceId });

  const linkedWorkspace = useMemo(
    () =>
      remoteWorkspaces.find(
        (ws) =>
          ws.local_workspace_id === workspaceId && ws.project_id === projectId
      ) ?? null,
    [remoteWorkspaces, workspaceId, projectId]
  );

  const linkedTaskId = linkedWorkspace?.task_id ?? null;
  const breadcrumbTaskId = routeState.taskId ?? linkedTaskId;

  const taskSimpleId = useMemo(() => {
    if (!breadcrumbTaskId) return null;
    return getTask(breadcrumbTaskId)?.simple_id ?? null;
  }, [breadcrumbTaskId, getTask]);

  const canonicalSession = findAppShellRecentSession(
    recentSessions,
    selectedSessionId
  );
  const workbenchHeader = deriveAgentWorkbenchHeader({
    canonicalSession:
      canonicalSession?.workspace_id === workspaceId
        ? canonicalSession
        : undefined,
    fallbackTitle: isNewSessionMode
      ? t('agentWorkbench.header.newSession', { defaultValue: 'New session' })
      : selectedSession?.name?.trim() ||
        t('agentWorkbench.header.untitledTask', {
          defaultValue: 'Untitled task',
        }),
  });
  const workspaceContextLabel = workspace
    ? buildWorkspaceContext({
        containerRef: workspace.container_ref,
        workingDir: selectedSession?.agent_working_dir,
        fallbackRepoName:
          reposQuery.repos.length === 1 ? reposQuery.repos[0].name : undefined,
        branch: workspace.branch,
        workspaceKind: workspace.workspace_kind,
        worktreeLabel: t('agentWorkbench.header.worktree', {
          defaultValue: 'Worktree',
        }),
      })
        .map((part) => part.label)
        .join(' · ')
    : undefined;

  const handleOpenTaskPanel = useCallback(() => {
    if (projectId && breadcrumbTaskId) {
      appNavigation.goToProjectTask(projectId, breadcrumbTaskId);
      return;
    }
    onClose();
  }, [projectId, breadcrumbTaskId, appNavigation, onClose]);

  const handleOpenWorkspaceView = useCallback(() => {
    appNavigation.goToWorkspace(workspaceId);
  }, [appNavigation, workspaceId]);

  const breadcrumbButtonClass =
    'min-h-8 min-w-0 rounded-sm px-half py-half text-sm text-normal truncate hover:bg-panel hover:text-high transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand [@media(pointer:coarse)]:min-h-11';
  const headerActionClass =
    'flex min-h-8 min-w-8 shrink-0 items-center justify-center rounded-sm text-low hover:text-normal hover:bg-panel transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand [@media(pointer:coarse)]:min-h-11 [@media(pointer:coarse)]:min-w-11';
  const isWorkspaceLoading = workspaceQuery.isLoading || reposQuery.isLoading;
  const workspaceState = deriveWorkspaceDetailState({
    hasWorkspaceId: Boolean(workspaceId),
    isLoading: isWorkspaceLoading,
    hasWorkspace: Boolean(workspace),
    error: workspaceQuery.error ?? reposQuery.error,
  });
  const { refetch: refetchWorkspace } = workspaceQuery;
  const { refetch: refetchRepos } = reposQuery;
  const retryWorkspace = useCallback(async () => {
    await Promise.all([refetchWorkspace(), refetchRepos()]);
  }, [refetchWorkspace, refetchRepos]);

  return (
    <ExecutionProcessesProvider
      key={`${hostId ?? 'local'}:${workspaceId}:${selectedSessionId ?? 'new'}`}
      sessionId={selectedSessionId}
    >
      <div className="relative flex h-full min-h-0 min-w-0 flex-1 flex-col bg-primary">
        <div className="flex min-w-0 items-center justify-between gap-base px-base py-half border-b shrink-0">
          <div className="flex min-w-0 flex-1 flex-col">
            <p
              className="min-w-0 truncate text-lg font-medium text-high"
              title={workbenchHeader.title}
            >
              {workbenchHeader.title}
            </p>
            <div className="flex min-w-0 items-center gap-half text-sm text-low">
              <button
                type="button"
                onClick={handleOpenTaskPanel}
                className={`${breadcrumbButtonClass} max-w-[45%] shrink-0`}
                aria-label={t('sessionDashboard.openWork', {
                  title: taskSimpleId ?? t('sessionDashboard.kinds.task'),
                  defaultValue: 'Open task: {{title}}',
                })}
              >
                {taskSimpleId ??
                  t('sessionDashboard.kinds.task', { defaultValue: 'Task' })}
              </button>
              {workspaceContextLabel && (
                <>
                  <span className="shrink-0" aria-hidden="true">
                    ·
                  </span>
                  <span
                    className="min-w-0 truncate font-ibm-plex-mono"
                    title={workspaceContextLabel}
                  >
                    {workspaceContextLabel}
                  </span>
                </>
              )}
            </div>
          </div>

          <div className="flex shrink-0 items-center gap-half">
            <button
              type="button"
              onClick={handleOpenWorkspaceView}
              className={headerActionClass}
              aria-label={t('sessionDashboard.openFullConversation', {
                defaultValue: 'Open full conversation',
              })}
              title={t('sessionDashboard.openFullConversation', {
                defaultValue: 'Open full conversation',
              })}
            >
              <ArrowsOutIcon className="size-icon-sm" weight="bold" />
            </button>
            <button
              type="button"
              onClick={onClose}
              className={headerActionClass}
              aria-label={t('sessionDashboard.closeConversation', {
                defaultValue: 'Close conversation',
              })}
            >
              <XIcon className="size-icon-sm" weight="bold" />
            </button>
          </div>
        </div>

        <WorkspaceConversationContent
          selectedWorkspace={workspace ?? null}
          selectedSession={selectedSession}
          selectedSessionId={selectedSessionId}
          sessions={sessions}
          repos={reposQuery.repos}
          isLoading={isWorkspaceLoading}
          workspaceState={workspaceState}
          isRetryingWorkspace={
            workspaceQuery.isFetching || reposQuery.isLoading
          }
          onRetryWorkspace={retryWorkspace}
          isSessionsLoading={isSessionsLoading}
          sessionsError={sessionsError}
          onRetrySessions={retrySessions}
          isNewSessionMode={isNewSessionMode}
          onSelectSession={selectSession}
          onStartNewSession={startNewSession}
          presentation="embedded"
          showSessionControls
        />
      </div>
    </ExecutionProcessesProvider>
  );
}

export function ProjectRightSidebarContainer() {
  const appNavigation = useAppNavigation();
  const {
    projectId,
    getTask,
    isLoading: isProjectLoading,
    tasksById,
  } = useProjectContext();
  const routeState = useCurrentKanbanRouteState();
  const { taskId, workspaceId, draftId, isWorkspaceCreateMode, hostId } =
    routeState;
  const taskComposerKey = useMemo(() => {
    if (!projectId) {
      return null;
    }

    return buildKanbanTaskComposerKey(hostId, projectId);
  }, [hostId, projectId]);
  const taskComposer = useKanbanTaskComposer(taskComposerKey);
  const isCreateMode = taskComposer !== null;
  // Kanban creation always starts with an Issue. Standalone creation belongs
  // to the workspace entrypoint, even when it selects this same project.
  useEffect(() => {
    if (!isWorkspaceCreateMode || taskId || !taskComposerKey) return;
    if (!taskComposer) openKanbanTaskComposer(taskComposerKey);
    appNavigation.goToProject(projectId, { replace: true });
  }, [
    isWorkspaceCreateMode,
    taskId,
    taskComposerKey,
    taskComposer,
    appNavigation,
    projectId,
  ]);
  const openTask = useCallback(
    (targetTaskId: string) => {
      if (!projectId) {
        return;
      }

      if (isCreateMode && taskComposerKey) {
        closeKanbanTaskComposer(taskComposerKey);
      }

      appNavigation.goToProjectTask(projectId, targetTaskId);
    },
    [projectId, isCreateMode, taskComposerKey, appNavigation]
  );
  const openTaskWorkspace = useCallback(
    (targetTaskId: string, targetWorkspaceId: string) => {
      if (!projectId) {
        return;
      }

      appNavigation.goToProjectTaskWorkspace(
        projectId,
        targetTaskId,
        targetWorkspaceId
      );
    },
    [projectId, appNavigation]
  );
  const closePanel = useCallback(() => {
    if (!projectId) {
      return;
    }

    if (isCreateMode && taskComposerKey) {
      closeKanbanTaskComposer(taskComposerKey);
    }

    appNavigation.goToProject(projectId);
  }, [projectId, isCreateMode, taskComposerKey, appNavigation]);
  const [expectedTaskId, setExpectedTaskId] = useState<string | null>(null);

  const markExpectedTask = useCallback((nextTaskId: string) => {
    setExpectedTaskId(nextTaskId);
  }, []);

  // Keep transient create expectations scoped to the current issue route only.
  useEffect(() => {
    if (!expectedTaskId) {
      return;
    }

    if (!taskId || taskId !== expectedTaskId) {
      setExpectedTaskId(null);
      return;
    }

    if (tasksById.has(expectedTaskId)) {
      setExpectedTaskId(null);
    }
  }, [expectedTaskId, taskId, tasksById]);

  const taskPanelResolution = useMemo<TaskPanelResolution | null>(() => {
    if (!taskId || isCreateMode || workspaceId || isWorkspaceCreateMode) {
      return null;
    }

    return resolveTaskPanelResolution({
      taskId,
      hasTask: tasksById.has(taskId),
      isProjectLoading,
      expectedTaskId,
    });
  }, [
    taskId,
    isCreateMode,
    workspaceId,
    isWorkspaceCreateMode,
    tasksById,
    isProjectLoading,
    expectedTaskId,
  ]);

  const rightPanelState = useMemo<RightPanelState>(() => {
    if (isCreateMode) {
      return { kind: 'create-issue' };
    }

    if (isWorkspaceCreateMode) {
      if (draftId) {
        return {
          kind: 'workspace-create',
          draftId,
          taskId,
        };
      }
      return { kind: 'closed' };
    }

    if (workspaceId) {
      return { kind: 'task-workspace', workspaceId };
    }

    if (taskId) {
      return {
        kind: 'task',
        taskId,
        resolution: taskPanelResolution ?? 'resolving',
      };
    }

    return { kind: 'closed' };
  }, [
    isWorkspaceCreateMode,
    draftId,
    taskId,
    workspaceId,
    isCreateMode,
    taskPanelResolution,
  ]);

  const handleOpenTaskFromCreate = useCallback(
    (targetTaskId: string) => {
      openTask(targetTaskId);
    },
    [openTask]
  );

  const handleWorkspaceCreated = useCallback(
    (createdWorkspaceId: string) => {
      if (taskId) {
        openTaskWorkspace(taskId, createdWorkspaceId);
        return;
      }

      appNavigation.goToWorkspace(createdWorkspaceId);
    },
    [taskId, openTaskWorkspace, appNavigation]
  );

  useEffect(() => {
    if (rightPanelState.kind !== 'task') {
      return;
    }

    if (rightPanelState.resolution !== 'missing') {
      return;
    }

    closePanel();
  }, [rightPanelState, closePanel]);

  if (rightPanelState.kind === 'workspace-create') {
    const linkedTaskId = rightPanelState.taskId;
    if (!linkedTaskId) return null;
    const linkedTaskSimpleId = linkedTaskId
      ? (getTask(linkedTaskId)?.simple_id ?? null)
      : null;

    return (
      <WorkspaceCreatePanel
        linkedTaskId={linkedTaskId}
        linkedTaskSimpleId={linkedTaskSimpleId}
        onOpenTask={handleOpenTaskFromCreate}
        onClose={closePanel}
      >
        <CreateModeProvider
          key={rightPanelState.draftId}
          draftId={rightPanelState.draftId}
        >
          <CreateChatBoxContainer
            onWorkspaceCreated={handleWorkspaceCreated}
            requiredLinkedTask={{
              taskId: linkedTaskId,
              remoteProjectId: projectId,
              simpleId: linkedTaskSimpleId ?? linkedTaskId,
              title: getTask(linkedTaskId)?.title,
            }}
          />
        </CreateModeProvider>
      </WorkspaceCreatePanel>
    );
  }

  if (rightPanelState.kind === 'task-workspace') {
    return (
      <WorkspaceSessionPanel
        workspaceId={rightPanelState.workspaceId}
        onClose={closePanel}
      />
    );
  }

  if (rightPanelState.kind === 'closed') {
    return null;
  }

  return (
    <KanbanTaskPanelContainer
      taskResolution={
        rightPanelState.kind === 'task' ? rightPanelState.resolution : null
      }
      onExpectTaskOpen={markExpectedTask}
    />
  );
}
