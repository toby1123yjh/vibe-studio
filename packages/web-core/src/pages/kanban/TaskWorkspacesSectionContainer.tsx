import { useMemo, useCallback } from 'react';
import { useParams } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { LinkIcon, PlusIcon } from '@phosphor-icons/react';
import { useProjectContext } from '@/shared/hooks/useProjectContext';
import { useAuth } from '@/shared/hooks/auth/useAuth';
import { useOrgContext } from '@/shared/hooks/useOrgContext';
import { useUserContext } from '@/shared/hooks/useUserContext';
import { useWorkspaceContext } from '@/shared/hooks/useWorkspaceContext';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useCurrentKanbanRouteState } from '@/shared/hooks/useCurrentKanbanRouteState';
import { useProjectWorkspaceCreateDraft } from '@/shared/hooks/useProjectWorkspaceCreateDraft';
import { workspacesApi } from '@/shared/lib/api';
import { getWorkspaceDefaults } from '@/shared/lib/workspaceDefaults';
import {
  buildLinkedTaskCreateState,
  buildLocalWorkspaceIdSet,
  buildWorkspaceCreateInitialState,
} from '@/shared/lib/workspaceCreateState';
import { ConfirmDialog } from '@vibe/ui/components/ConfirmDialog';
import { DeleteWorkspaceDialog } from '@vibe/ui/components/DeleteWorkspaceDialog';
import type { WorkspaceWithStats } from '@vibe/ui/components/TaskWorkspaceCard';
import { TaskWorkspacesSection } from '@vibe/ui/components/TaskWorkspacesSection';
import type { SectionAction } from '@vibe/ui/components/CollapsibleSectionHeader';

interface TaskWorkspacesSectionContainerProps {
  taskId: string;
}

/**
 * Container component for the workspaces section.
 * Fetches workspace data from ProjectContext and transforms it for display.
 */
export function TaskWorkspacesSectionContainer({
  taskId,
}: TaskWorkspacesSectionContainerProps) {
  const { t } = useTranslation('common');
  const { projectId } = useParams({ strict: false });
  const appNavigation = useAppNavigation();
  const routeState = useCurrentKanbanRouteState();
  const { openWorkspaceCreateFromState } = useProjectWorkspaceCreateDraft();
  const { userId } = useAuth();
  const { workspaces } = useUserContext();

  const {
    pullRequests,
    getTask,
    getWorkspacesForTask,
    tasks,
    isLoading: projectLoading,
  } = useProjectContext();
  const { activeWorkspaces, archivedWorkspaces } = useWorkspaceContext();
  const { membersWithProfilesById, isLoading: orgLoading } = useOrgContext();

  const localWorkspacesById = useMemo(() => {
    const map = new Map<string, (typeof activeWorkspaces)[number]>();

    for (const workspace of activeWorkspaces) {
      map.set(workspace.id, workspace);
    }

    for (const workspace of archivedWorkspaces) {
      map.set(workspace.id, workspace);
    }

    return map;
  }, [activeWorkspaces, archivedWorkspaces]);

  // Get workspaces for the issue, with PR info
  const workspacesWithStats: WorkspaceWithStats[] = useMemo(() => {
    const rawWorkspaces = getWorkspacesForTask(taskId);

    return rawWorkspaces.map((workspace) => {
      const localWorkspace = workspace.local_workspace_id
        ? localWorkspacesById.get(workspace.local_workspace_id)
        : undefined;

      // Find all linked PRs for this workspace
      const linkedPrs = pullRequests
        .filter((pr) => pr.workspace_id === workspace.id)
        .map((pr) => ({
          number: pr.number,
          url: pr.url,
          status: pr.status as 'open' | 'merged' | 'closed',
        }));

      // Get owner
      const owner =
        membersWithProfilesById.get(workspace.owner_user_id) ?? null;

      return {
        id: workspace.id,
        localWorkspaceId: workspace.local_workspace_id,
        name: workspace.name,
        archived: workspace.archived,
        filesChanged: workspace.files_changed ?? 0,
        linesAdded: workspace.lines_added ?? 0,
        linesRemoved: workspace.lines_removed ?? 0,
        prs: linkedPrs,
        owner,
        updatedAt: workspace.updated_at,
        isOwnedByCurrentUser: workspace.owner_user_id === userId,
        isRunning: localWorkspace?.isRunning,
        hasPendingApproval: localWorkspace?.hasPendingApproval,
        hasRunningDevServer: localWorkspace?.hasRunningDevServer,
        hasUnseenActivity: localWorkspace?.hasUnseenActivity,
        latestProcessCompletedAt: localWorkspace?.latestProcessCompletedAt,
        latestProcessStatus: localWorkspace?.latestProcessStatus,
      };
    });
  }, [
    taskId,
    getWorkspacesForTask,
    pullRequests,
    membersWithProfilesById,
    userId,
    localWorkspacesById,
  ]);

  const isLoading = projectLoading || orgLoading;
  const shouldAnimateCreateButton = useMemo(() => {
    if (tasks.length !== 1) {
      return false;
    }

    return tasks.every((task) => getWorkspacesForTask(task.id).length === 0);
  }, [tasks, getWorkspacesForTask]);

  // Handle clicking '+' to create and link a new workspace directly
  const handleAddWorkspace = useCallback(async () => {
    if (!projectId) {
      return;
    }

    const task = getTask(taskId);
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
      taskId,
    });
    if (!draftId) {
      await ConfirmDialog.show({
        title: t('common:error'),
        message: t(
          'workspaces.createDraftError',
          'Failed to prepare workspace draft. Please try again.'
        ),
        confirmText: t('common:ok'),
        showCancelButton: false,
      });
    }
  }, [
    projectId,
    openWorkspaceCreateFromState,
    getTask,
    taskId,
    activeWorkspaces,
    archivedWorkspaces,
    workspaces,
    routeState.hostId,
    t,
  ]);

  // Handle clicking link action to link an existing workspace
  const handleLinkWorkspace = useCallback(async () => {
    if (!projectId) {
      return;
    }

    const { WorkspaceSelectionDialog } = await import(
      '@/shared/dialogs/command-bar/WorkspaceSelectionDialog'
    );
    await WorkspaceSelectionDialog.show({
      projectId,
      taskId,
      hostId: routeState.hostId,
    });
  }, [projectId, taskId, routeState.hostId]);

  // Handle clicking a workspace card to open it
  const handleWorkspaceClick = useCallback(
    (localWorkspaceId: string | null) => {
      if (projectId && localWorkspaceId) {
        appNavigation.goToProjectTaskWorkspace(
          projectId,
          taskId,
          localWorkspaceId
        );
      }
    },
    [projectId, taskId, appNavigation]
  );

  // Handle unlinking a workspace from the issue
  const handleUnlinkWorkspace = useCallback(
    async (localWorkspaceId: string) => {
      const result = await ConfirmDialog.show({
        title: t('workspaces.unlinkFromIssue'),
        message: t('workspaces.unlinkConfirmMessage'),
        confirmText: t('workspaces.unlink'),
        variant: 'destructive',
      });

      if (result === 'confirmed') {
        try {
          await workspacesApi.unlinkFromTask(localWorkspaceId);
        } catch (error) {
          ConfirmDialog.show({
            title: t('common:error'),
            message:
              error instanceof Error
                ? error.message
                : t('workspaces.unlinkError'),
            confirmText: t('common:ok'),
            showCancelButton: false,
          });
        }
      }
    },
    [t]
  );

  // Handle deleting a workspace (unlinks first, then deletes local)
  const handleDeleteWorkspace = useCallback(
    async (localWorkspaceId: string) => {
      const localWorkspace = localWorkspacesById.get(localWorkspaceId);
      if (!localWorkspace) {
        ConfirmDialog.show({
          title: t('common:error'),
          message: t('workspaces.deleteError'),
          confirmText: t('common:ok'),
          showCancelButton: false,
        });
        return;
      }

      const result = await DeleteWorkspaceDialog.show({
        branchName: localWorkspace.branch,
        hasOpenPR:
          workspacesWithStats
            .find(
              (workspace) => workspace.localWorkspaceId === localWorkspaceId
            )
            ?.prs.some((pr) => pr.status === 'open') ?? false,
        isLinkedToTask: true,
        linkedTaskSimpleId: getTask(taskId)?.simple_id,
      });

      if (result.action !== 'confirmed') {
        return;
      }

      try {
        // Delete local workspace first
        await workspacesApi.delete(localWorkspaceId, result.deleteBranches);
        // Unlink from remote after successful deletion
        if (result.unlinkFromTask) {
          await workspacesApi.unlinkFromTask(localWorkspaceId);
        }
      } catch (error) {
        ConfirmDialog.show({
          title: t('common:error'),
          message:
            error instanceof Error
              ? error.message
              : t('workspaces.deleteError'),
          confirmText: t('common:ok'),
          showCancelButton: false,
        });
      }
    },
    [localWorkspacesById, workspacesWithStats, t, taskId, getTask]
  );

  // Actions for the section header
  const actions: SectionAction[] = useMemo(
    () => [
      {
        icon: PlusIcon,
        onClick: handleAddWorkspace,
      },
      {
        icon: LinkIcon,
        onClick: handleLinkWorkspace,
      },
    ],
    [handleAddWorkspace, handleLinkWorkspace]
  );

  return (
    <TaskWorkspacesSection
      workspaces={workspacesWithStats}
      isLoading={isLoading}
      actions={actions}
      onWorkspaceClick={handleWorkspaceClick}
      onCreateWorkspace={handleAddWorkspace}
      onUnlinkWorkspace={handleUnlinkWorkspace}
      onDeleteWorkspace={handleDeleteWorkspace}
      shouldAnimateCreateButton={shouldAnimateCreateButton}
    />
  );
}
