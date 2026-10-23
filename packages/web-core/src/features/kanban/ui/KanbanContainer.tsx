import {
  useMemo,
  useCallback,
  useState,
  useEffect,
  useRef,
  type MouseEvent,
} from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectContext } from '@/shared/hooks/useProjectContext';
import { useOrgContext } from '@/shared/hooks/useOrgContext';
import { useWorkspaceContext } from '@/shared/hooks/useWorkspaceContext';
import { useActions } from '@/shared/hooks/useActions';
import { useAuth } from '@/shared/hooks/auth/useAuth';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useIsMobile } from '@/shared/hooks/useIsMobile';
import { useProjectWorkflowAttempts } from '@/shared/hooks/useWorkflowAttempts';
import { cn } from '@/shared/lib/utils';
import { useCurrentKanbanRouteState } from '@/shared/hooks/useCurrentKanbanRouteState';
import {
  useUiPreferencesStore,
  resolveKanbanProjectState,
  KANBAN_ASSIGNEE_FILTER_VALUES,
  KANBAN_PROJECT_VIEW_IDS,
  type KanbanFilterState,
  type KanbanSortField,
} from '@/shared/stores/useUiPreferencesStore';
import {
  useKanbanFilters,
  PRIORITY_ORDER,
} from '../model/hooks/useKanbanFilters';
import {
  bulkUpdateTasks,
  type BulkUpdateTaskItem,
} from '@/shared/lib/remoteApi';
import { PlusIcon, DotsThreeIcon } from '@phosphor-icons/react';
import { Actions } from '@/shared/actions';
import {
  buildKanbanTaskComposerKey,
  closeKanbanTaskComposer,
  openKanbanTaskComposer,
  type ProjectTaskCreateOptions,
  useKanbanTaskComposer,
} from '@/shared/stores/useKanbanTaskComposerStore';
import type {
  OrganizationMemberWithProfile,
  WorkflowAttemptResponse,
} from 'shared/types';
import {
  KanbanProvider,
  KanbanBoard,
  KanbanCard,
  KanbanCards,
  KanbanHeader,
  type DropResult,
} from '@vibe/ui/components/KanbanBoard';
import { KanbanCardContent } from '@vibe/ui/components/KanbanCardContent';
import {
  TaskWorkspaceCard,
  type WorkspaceWithStats,
  type WorkspacePr,
} from '@vibe/ui/components/TaskWorkspaceCard';
import {
  TaskWorkflowAttemptCard,
  type TaskWorkflowAttemptCardData,
} from '@vibe/ui/components/TaskWorkflowAttemptCard';
import { resolveRelationshipsForTask } from '@/shared/lib/resolveRelationships';
import { KanbanFilterBar } from '@vibe/ui/components/KanbanFilterBar';
import { ViewNavTabs } from '@vibe/ui/components/ViewNavTabs';
import { TaskListView } from '@vibe/ui/components/TaskListView';
import { CommandBarDialog } from '@/shared/dialogs/command-bar/CommandBarDialog';
import { KanbanFiltersDialog } from '@/shared/dialogs/kanban/KanbanFiltersDialog';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@vibe/ui/components/Dropdown';
import { SearchableTagDropdownContainer } from '@/shared/components/SearchableTagDropdownContainer';
import { ProjectWorkspaceDefaultContext } from '@/shared/components/ProjectWorkspaceDefaultContext';
import type { TaskPriority } from 'shared/remote-types';
import { useTaskMultiSelect } from '@/shared/hooks/useTaskMultiSelect';
import { useTaskSelectionStore } from '@/shared/stores/useTaskSelectionStore';
import { BulkActionBarContainer } from './BulkActionBarContainer';
import {
  getWorkflowAttemptWorkspaceIds,
  workflowAttemptStatusLabel,
  workflowAttemptStatusTone,
} from '@/features/workflow/model/taskAttempt';

const areStringSetsEqual = (left: string[], right: string[]): boolean => {
  if (left.length !== right.length) {
    return false;
  }

  const rightSet = new Set(right);
  return left.every((value) => rightSet.has(value));
};

const areKanbanFiltersEqual = (
  left: KanbanFilterState,
  right: KanbanFilterState
): boolean => {
  if (left.searchQuery.trim() !== right.searchQuery.trim()) {
    return false;
  }

  if (!areStringSetsEqual(left.priorities, right.priorities)) {
    return false;
  }

  if (!areStringSetsEqual(left.assigneeIds, right.assigneeIds)) {
    return false;
  }

  if (!areStringSetsEqual(left.tagIds, right.tagIds)) {
    return false;
  }

  return (
    left.sortField === right.sortField &&
    left.sortDirection === right.sortDirection
  );
};

function workflowAttemptToKanbanCard(
  attempt: WorkflowAttemptResponse
): TaskWorkflowAttemptCardData {
  return {
    id: attempt.id,
    title: attempt.name || 'Workflow attempt',
    subtitle: attempt.latest_run_id
      ? `Latest run ${attempt.latest_run_id.slice(0, 8)}`
      : 'Workflow draft ready',
    statusLabel: workflowAttemptStatusLabel(attempt.status),
    statusTone: workflowAttemptStatusTone(attempt.status),
  };
}

function LoadingState() {
  const { t } = useTranslation('common');
  return (
    <div className="flex items-center justify-center h-full">
      <p className="text-low">{t('states.loading')}</p>
    </div>
  );
}

/**
 * KanbanContainer displays the kanban board using data from ProjectContext and OrgContext.
 * Must be rendered within both OrgProvider and ProjectProvider.
 */
export function KanbanContainer() {
  const isMobile = useIsMobile();
  const { t } = useTranslation('common');
  const appNavigation = useAppNavigation();
  const routeState = useCurrentKanbanRouteState();

  // Get data from contexts (set up by WorkspacesLayout)
  const {
    projectId,
    tasks,
    statuses,
    tags,
    taskAssignees,
    taskTags,
    taskRelationships,
    getTagObjectsForTask,
    getTagsForTask,
    getPullRequestsForTask,
    getWorkspacesForTask,
    getRelationshipsForTask,
    tasksById,
    insertTaskTag,
    removeTaskTag,
    insertTag,
    pullRequests,
    isLoading: projectLoading,
  } = useProjectContext();

  const {
    projects,
    membersWithProfilesById,
    isLoading: orgLoading,
  } = useOrgContext();
  const { activeWorkspaces } = useWorkspaceContext();
  const { userId } = useAuth();

  // Get project name by finding the project matching current projectId
  const projectName = projects.find((p) => p.id === projectId)?.name ?? '';

  const selectedKanbanTaskId = routeState.taskId;
  const taskComposerKey = useMemo(
    () => buildKanbanTaskComposerKey(routeState.hostId, projectId),
    [routeState.hostId, projectId]
  );
  const taskComposer = useKanbanTaskComposer(taskComposerKey);
  const isTaskComposerOpen = taskComposer !== null;
  const openTask = useCallback(
    (taskId: string) => {
      if (isTaskComposerOpen) {
        closeKanbanTaskComposer(taskComposerKey);
      }

      appNavigation.goToProjectTask(projectId, taskId);
    },
    [isTaskComposerOpen, taskComposerKey, appNavigation, projectId]
  );
  const openTaskWorkspace = useCallback(
    (taskId: string, workspaceAttemptId: string) => {
      appNavigation.goToProjectTaskWorkspace(
        projectId,
        taskId,
        workspaceAttemptId
      );
    },
    [appNavigation, projectId]
  );
  const openTaskWorkflowAttempt = useCallback(
    (workflowId: string) => {
      appNavigation.goToProjectWorkflowEdit(projectId, workflowId);
    },
    [appNavigation, projectId]
  );
  const startCreate = useCallback(
    (options?: ProjectTaskCreateOptions) => {
      openKanbanTaskComposer(taskComposerKey, options);
    },
    [taskComposerKey]
  );

  // Get setter and executor from ActionsContext
  const {
    setDefaultCreateStatusId,
    executeAction,
    openPrioritySelection,
    openAssigneeSelection,
  } = useActions();
  const openProjectsGuide = useCallback(() => {
    executeAction(Actions.ProjectsGuide);
  }, [executeAction]);

  const projectViewSelection = useUiPreferencesStore(
    (s) => s.kanbanProjectViewSelections[projectId]
  );
  const projectViewPreferencesById = useUiPreferencesStore(
    (s) => s.kanbanProjectViewPreferences[projectId]
  );
  const setKanbanProjectView = useUiPreferencesStore(
    (s) => s.setKanbanProjectView
  );
  const setKanbanProjectViewFilters = useUiPreferencesStore(
    (s) => s.setKanbanProjectViewFilters
  );
  const setKanbanProjectViewShowSubTasks = useUiPreferencesStore(
    (s) => s.setKanbanProjectViewShowSubTasks
  );
  const setKanbanProjectViewShowWorkspaces = useUiPreferencesStore(
    (s) => s.setKanbanProjectViewShowWorkspaces
  );
  const setKanbanProjectViewHideBlocked = useUiPreferencesStore(
    (s) => s.setKanbanProjectViewHideBlocked
  );
  const clearKanbanProjectViewPreferences = useUiPreferencesStore(
    (s) => s.clearKanbanProjectViewPreferences
  );
  const resolvedProjectState = useMemo(
    () => resolveKanbanProjectState(projectViewSelection),
    [projectViewSelection]
  );
  const {
    activeViewId,
    filters: defaultKanbanFilters,
    showSubTasks: defaultShowSubTasks,
    showWorkspaces: defaultShowWorkspaces,
    hideBlocked: defaultHideBlocked,
  } = resolvedProjectState;
  const projectViewPreferences = projectViewPreferencesById?.[activeViewId];
  const kanbanFilters = projectViewPreferences?.filters ?? defaultKanbanFilters;
  const showSubTasks =
    projectViewPreferences?.showSubTasks ?? defaultShowSubTasks;
  const showWorkspaces =
    projectViewPreferences?.showWorkspaces ?? defaultShowWorkspaces;
  const hideBlocked = projectViewPreferences?.hideBlocked ?? defaultHideBlocked;
  const { data: projectWorkflowAttemptData } = useProjectWorkflowAttempts(
    projectId,
    { enabled: showWorkspaces }
  );

  const hasActiveFilters = useMemo(
    () =>
      !areKanbanFiltersEqual(kanbanFilters, defaultKanbanFilters) ||
      showSubTasks !== defaultShowSubTasks ||
      showWorkspaces !== defaultShowWorkspaces ||
      hideBlocked !== defaultHideBlocked,
    [
      kanbanFilters,
      defaultKanbanFilters,
      showSubTasks,
      defaultShowSubTasks,
      showWorkspaces,
      defaultShowWorkspaces,
      hideBlocked,
      defaultHideBlocked,
    ]
  );
  const shouldAnimateCreateButton = tasks.length === 0;

  // Compute resolved status IDs for the blocked filter.
  // A blocking issue is considered resolved when it's in:
  // - The last visible status (rightmost kanban column, e.g. "Done")
  // - Any hidden status (terminal states like "Cancelled" that don't appear as columns)
  const doneStatusIds = useMemo(() => {
    const ids = new Set<string>();
    for (const s of statuses) {
      if (s.hidden) ids.add(s.id);
    }
    const sorted = statuses
      .filter((s) => !s.hidden)
      .sort((a, b) => a.sort_order - b.sort_order);
    const lastVisible = sorted[sorted.length - 1];
    if (lastVisible) ids.add(lastVisible.id);
    return ids;
  }, [statuses]);

  const { filteredTasks } = useKanbanFilters({
    tasks,
    taskAssignees,
    taskTags,
    taskRelationships,
    tasksById,
    doneStatusIds,
    filters: kanbanFilters,
    showSubTasks,
    hideBlocked,
    currentUserId: userId,
  });

  const setKanbanSearchQuery = useCallback(
    (searchQuery: string) => {
      setKanbanProjectViewFilters(projectId, activeViewId, {
        ...kanbanFilters,
        searchQuery,
      });
    },
    [activeViewId, kanbanFilters, projectId, setKanbanProjectViewFilters]
  );

  const setKanbanPriorities = useCallback(
    (priorities: TaskPriority[]) => {
      setKanbanProjectViewFilters(projectId, activeViewId, {
        ...kanbanFilters,
        priorities,
      });
    },
    [activeViewId, kanbanFilters, projectId, setKanbanProjectViewFilters]
  );

  const setKanbanAssignees = useCallback(
    (assigneeIds: string[]) => {
      setKanbanProjectViewFilters(projectId, activeViewId, {
        ...kanbanFilters,
        assigneeIds,
      });
    },
    [activeViewId, kanbanFilters, projectId, setKanbanProjectViewFilters]
  );

  const setKanbanTags = useCallback(
    (tagIds: string[]) => {
      setKanbanProjectViewFilters(projectId, activeViewId, {
        ...kanbanFilters,
        tagIds,
      });
    },
    [activeViewId, kanbanFilters, projectId, setKanbanProjectViewFilters]
  );

  const setKanbanSort = useCallback(
    (sortField: KanbanSortField, sortDirection: 'asc' | 'desc') => {
      setKanbanProjectViewFilters(projectId, activeViewId, {
        ...kanbanFilters,
        sortField,
        sortDirection,
      });
    },
    [activeViewId, kanbanFilters, projectId, setKanbanProjectViewFilters]
  );

  const setShowSubTasks = useCallback(
    (show: boolean) => {
      setKanbanProjectViewShowSubTasks(projectId, activeViewId, show);
    },
    [activeViewId, projectId, setKanbanProjectViewShowSubTasks]
  );

  const setShowWorkspaces = useCallback(
    (show: boolean) => {
      setKanbanProjectViewShowWorkspaces(projectId, activeViewId, show);
    },
    [activeViewId, projectId, setKanbanProjectViewShowWorkspaces]
  );

  const setHideBlocked = useCallback(
    (hide: boolean) => {
      setKanbanProjectViewHideBlocked(projectId, activeViewId, hide);
    },
    [activeViewId, projectId, setKanbanProjectViewHideBlocked]
  );

  const clearKanbanFilters = useCallback(() => {
    clearKanbanProjectViewPreferences(projectId, activeViewId);
  }, [activeViewId, clearKanbanProjectViewPreferences, projectId]);

  const handleKanbanProjectViewChange = useCallback(
    (viewId: string) => {
      setKanbanProjectView(projectId, viewId);
    },
    [projectId, setKanbanProjectView]
  );
  const kanbanViewMode = useUiPreferencesStore((s) => s.kanbanViewMode);
  const listViewStatusFilter = useUiPreferencesStore(
    (s) => s.listViewStatusFilter
  );
  const setKanbanViewMode = useUiPreferencesStore((s) => s.setKanbanViewMode);
  const setListViewStatusFilter = useUiPreferencesStore(
    (s) => s.setListViewStatusFilter
  );
  // Reset view mode when navigating projects
  const prevProjectIdRef = useRef<string | null>(null);

  // Track when drag-drop sync is in progress to prevent flicker
  const isSyncingRef = useRef(false);

  useEffect(() => {
    if (
      prevProjectIdRef.current !== null &&
      prevProjectIdRef.current !== projectId
    ) {
      setKanbanViewMode('kanban');
      setListViewStatusFilter(null);
    }

    prevProjectIdRef.current = projectId;
  }, [projectId, setKanbanViewMode, setListViewStatusFilter]);

  // Sort all statuses for display settings
  const sortedStatuses = useMemo(
    () => [...statuses].sort((a, b) => a.sort_order - b.sort_order),
    [statuses]
  );

  // Filter statuses: visible (non-hidden) for kanban, hidden for tabs
  const visibleStatuses = useMemo(
    () => sortedStatuses.filter((s) => !s.hidden),
    [sortedStatuses]
  );

  // Map status ID to 1-based column index for sort_order calculation
  const statusColumnIndexMap = useMemo(() => {
    const map = new Map<string, number>();
    visibleStatuses.forEach((status, index) => {
      map.set(status.id, index + 1);
    });
    return map;
  }, [visibleStatuses]);

  const hiddenStatuses = useMemo(
    () => sortedStatuses.filter((s) => s.hidden),
    [sortedStatuses]
  );

  const defaultCreateStatusId = useMemo(() => {
    if (kanbanViewMode === 'kanban') {
      return visibleStatuses[0]?.id;
    }
    if (listViewStatusFilter) {
      return listViewStatusFilter;
    }
    return sortedStatuses[0]?.id;
  }, [kanbanViewMode, visibleStatuses, listViewStatusFilter, sortedStatuses]);

  // Update default create status for command bar based on current tab
  useEffect(() => {
    setDefaultCreateStatusId(defaultCreateStatusId);
  }, [defaultCreateStatusId, setDefaultCreateStatusId]);

  const createAssigneeIds = useMemo(() => {
    const assigneeIds = new Set<string>();

    for (const assigneeId of kanbanFilters.assigneeIds) {
      if (assigneeId === KANBAN_ASSIGNEE_FILTER_VALUES.UNASSIGNED) {
        continue;
      }

      if (assigneeId === KANBAN_ASSIGNEE_FILTER_VALUES.SELF) {
        if (userId) {
          assigneeIds.add(userId);
        }
        continue;
      }

      assigneeIds.add(assigneeId);
    }

    return [...assigneeIds];
  }, [kanbanFilters.assigneeIds, userId]);

  // Get statuses to display in list view (all or filtered to one)
  const listViewStatuses = useMemo(() => {
    if (listViewStatusFilter) {
      return sortedStatuses.filter((s) => s.id === listViewStatusFilter);
    }
    return sortedStatuses;
  }, [sortedStatuses, listViewStatusFilter]);

  // Track items as arrays of IDs grouped by status
  const [items, setItems] = useState<Record<string, string[]>>({});
  const [isFiltersDialogOpen, setIsFiltersDialogOpen] = useState(false);

  // Sync items from filtered issues when they change
  useEffect(() => {
    // Skip rebuild during drag-drop sync to prevent flicker
    if (isSyncingRef.current) {
      return;
    }

    const { sortField, sortDirection } = kanbanFilters;
    const grouped: Record<string, string[]> = {};

    for (const status of statuses) {
      // Filter issues for this status
      let statusTasks = filteredTasks.filter((i) => i.status_id === status.id);

      // Sort within column based on user preference
      statusTasks = [...statusTasks].sort((a, b) => {
        let comparison = 0;
        switch (sortField) {
          case 'priority':
            comparison =
              (a.priority ? PRIORITY_ORDER[a.priority] : Infinity) -
              (b.priority ? PRIORITY_ORDER[b.priority] : Infinity);
            break;
          case 'created_at':
            comparison =
              new Date(a.created_at).getTime() -
              new Date(b.created_at).getTime();
            break;
          case 'updated_at':
            comparison =
              new Date(a.updated_at).getTime() -
              new Date(b.updated_at).getTime();
            break;
          case 'title':
            comparison = a.title.localeCompare(b.title);
            break;
          case 'sort_order':
          default:
            comparison = a.sort_order - b.sort_order;
        }
        return sortDirection === 'desc' ? -comparison : comparison;
      });

      grouped[status.id] = statusTasks.map((i) => i.id);
    }
    setItems(grouped);
  }, [filteredTasks, statuses, kanbanFilters]);

  // Create a lookup map for issue data
  const taskMap = useMemo(() => {
    const map: Record<string, (typeof tasks)[0]> = {};
    for (const task of tasks) {
      map[task.id] = task;
    }
    return map;
  }, [tasks]);

  // Create a lookup map for issue assignees (issue_id -> OrganizationMemberWithProfile[])
  const taskAssigneesMap = useMemo(() => {
    const map: Record<string, OrganizationMemberWithProfile[]> = {};
    for (const assignee of taskAssignees) {
      const member = membersWithProfilesById.get(assignee.user_id);
      if (member) {
        if (!map[assignee.task_id]) {
          map[assignee.task_id] = [];
        }
        map[assignee.task_id].push(member);
      }
    }
    return map;
  }, [taskAssignees, membersWithProfilesById]);

  const membersWithProfiles = useMemo(
    () => [...membersWithProfilesById.values()],
    [membersWithProfilesById]
  );

  const localWorkspacesById = useMemo(() => {
    const map = new Map<string, (typeof activeWorkspaces)[number]>();

    for (const workspace of activeWorkspaces) {
      map.set(workspace.id, workspace);
    }

    return map;
  }, [activeWorkspaces]);

  const prsByWorkspaceId = useMemo(() => {
    const map = new Map<string, WorkspacePr[]>();

    for (const pr of pullRequests) {
      if (!pr.workspace_id) continue;

      const prs = map.get(pr.workspace_id) ?? [];
      prs.push({
        number: pr.number,
        url: pr.url,
        status: pr.status as 'open' | 'merged' | 'closed',
      });
      map.set(pr.workspace_id, prs);
    }

    return map;
  }, [pullRequests]);

  const projectWorkflowAttempts = useMemo(
    () => projectWorkflowAttemptData?.attempts ?? [],
    [projectWorkflowAttemptData?.attempts]
  );

  const workflowWorkspaceIds = useMemo(
    () => getWorkflowAttemptWorkspaceIds(projectWorkflowAttempts),
    [projectWorkflowAttempts]
  );

  const workflowAttemptsByTaskId = useMemo(() => {
    if (!showWorkspaces) {
      return new Map<string, WorkflowAttemptResponse[]>();
    }

    const map = new Map<string, WorkflowAttemptResponse[]>();
    for (const attempt of projectWorkflowAttempts) {
      const attempts = map.get(attempt.task_id) ?? [];
      attempts.push(attempt);
      map.set(attempt.task_id, attempts);
    }
    return map;
  }, [projectWorkflowAttempts, showWorkspaces]);

  const workspacesByTaskId = useMemo(() => {
    if (!showWorkspaces) {
      return new Map<string, WorkspaceWithStats[]>();
    }

    const map = new Map<string, WorkspaceWithStats[]>();

    for (const task of tasks) {
      const nonArchivedWorkspaces = getWorkspacesForTask(task.id)
        .filter(
          (workspace) =>
            !workspace.archived &&
            !workflowWorkspaceIds.has(workspace.id) &&
            !!workspace.local_workspace_id &&
            localWorkspacesById.has(workspace.local_workspace_id)
        )
        .map((workspace) => {
          const localWorkspace = localWorkspacesById.get(
            workspace.local_workspace_id!
          );

          return {
            id: workspace.id,
            localWorkspaceId: workspace.local_workspace_id,
            name: workspace.name,
            archived: workspace.archived,
            filesChanged: workspace.files_changed ?? 0,
            linesAdded: workspace.lines_added ?? 0,
            linesRemoved: workspace.lines_removed ?? 0,
            prs: prsByWorkspaceId.get(workspace.id) ?? [],
            owner: membersWithProfilesById.get(workspace.owner_user_id) ?? null,
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

      if (nonArchivedWorkspaces.length > 0) {
        map.set(task.id, nonArchivedWorkspaces);
      }
    }

    return map;
  }, [
    showWorkspaces,
    tasks,
    getWorkspacesForTask,
    workflowWorkspaceIds,
    localWorkspacesById,
    prsByWorkspaceId,
    membersWithProfilesById,
    userId,
  ]);

  // Calculate sort_order based on column index and issue position
  // Formula: 1000 * [COLUMN_INDEX] + [ISSUE_INDEX] (both 1-based)
  const calculateSortOrder = useCallback(
    (statusId: string, taskIndex: number): number => {
      const columnIndex = statusColumnIndexMap.get(statusId) ?? 1;
      return 1000 * columnIndex + (taskIndex + 1);
    },
    [statusColumnIndexMap]
  );

  // Simple onDragEnd handler - the library handles all visual movement
  const handleDragEnd = useCallback(
    (result: DropResult) => {
      const { source, destination } = result;

      // Dropped outside a valid droppable
      if (!destination) return;

      // No movement
      if (
        source.droppableId === destination.droppableId &&
        source.index === destination.index
      ) {
        return;
      }

      const isManualSort = kanbanFilters.sortField === 'sort_order';

      // Block within-column reordering when not in manual sort mode
      // (cross-column moves are always allowed for status changes)
      if (source.droppableId === destination.droppableId && !isManualSort) {
        return;
      }

      const sourceId = source.droppableId;
      const destId = destination.droppableId;
      const isCrossColumn = sourceId !== destId;

      // Update local state and capture new items for bulk update
      let newItems: Record<string, string[]> = {};
      setItems((prev) => {
        const sourceItems = [...(prev[sourceId] ?? [])];
        const [moved] = sourceItems.splice(source.index, 1);

        if (!isCrossColumn) {
          // Within-column reorder
          sourceItems.splice(destination.index, 0, moved);
          newItems = { ...prev, [sourceId]: sourceItems };
        } else {
          // Cross-column move
          const destItems = [...(prev[destId] ?? [])];
          destItems.splice(destination.index, 0, moved);
          newItems = {
            ...prev,
            [sourceId]: sourceItems,
            [destId]: destItems,
          };
        }
        return newItems;
      });

      // Build bulk updates for all issues in affected columns
      const updates: BulkUpdateTaskItem[] = [];

      // Always update destination column
      const destTaskIds = newItems[destId] ?? [];
      destTaskIds.forEach((taskId, index) => {
        updates.push({
          id: taskId,
          changes: {
            status_id: destId,
            sort_order: calculateSortOrder(destId, index),
          },
        });
      });

      // Update source column if cross-column move
      if (isCrossColumn) {
        const sourceTaskIds = newItems[sourceId] ?? [];
        sourceTaskIds.forEach((taskId, index) => {
          updates.push({
            id: taskId,
            changes: {
              sort_order: calculateSortOrder(sourceId, index),
            },
          });
        });
      }

      // Perform bulk update
      isSyncingRef.current = true;
      bulkUpdateTasks(updates)
        .catch((err) => {
          console.error('Failed to bulk update sort order:', err);
        })
        .finally(() => {
          // Delay clearing flag to let Electric sync complete
          setTimeout(() => {
            isSyncingRef.current = false;
          }, 500);
        });
    },
    [kanbanFilters.sortField, calculateSortOrder]
  );

  // Multi-select support
  const {
    selectedTaskIds,
    isMultiSelectActive,
    handleTaskClick,
    handleCheckboxChange,
    clearSelection,
  } = useTaskMultiSelect();
  const setOrderedTaskIds = useTaskSelectionStore((s) => s.setOrderedTaskIds);
  const setAnchor = useTaskSelectionStore((s) => s.setAnchor);

  // Compute ordered issue IDs for range selection
  const orderedTaskIds = useMemo(() => {
    const statusOrder =
      kanbanViewMode === 'kanban' ? visibleStatuses : listViewStatuses;
    return statusOrder.flatMap((status) => items[status.id] ?? []);
  }, [kanbanViewMode, visibleStatuses, listViewStatuses, items]);

  // Keep the store's ordered IDs in sync
  useEffect(() => {
    setOrderedTaskIds(orderedTaskIds);
  }, [orderedTaskIds, setOrderedTaskIds]);

  // Clear multi-selection when project or view mode changes
  useEffect(() => {
    clearSelection();
  }, [projectId, kanbanViewMode, clearSelection]);

  // Keep anchor in sync with the currently opened issue (e.g. from URL on
  // page load) so Shift/Cmd+Click on another issue includes it.
  useEffect(() => {
    if (selectedKanbanTaskId) {
      setAnchor(selectedKanbanTaskId);
    }
  }, [selectedKanbanTaskId, setAnchor]);

  const handleCardClick = useCallback(
    (taskId: string, e?: MouseEvent) => {
      if (e && (e.metaKey || e.ctrlKey || e.shiftKey)) {
        handleTaskClick(taskId, e);
      } else {
        if (selectedTaskIds.size > 0) {
          clearSelection();
        }
        // Set as anchor so Shift+Click from this issue works
        setAnchor(taskId);
        openTask(taskId);
      }
    },
    [openTask, handleTaskClick, selectedTaskIds.size, clearSelection, setAnchor]
  );

  const handleAddTask = useCallback(
    (statusId?: string) => {
      const createPayload = {
        statusId: statusId ?? defaultCreateStatusId,
        ...(createAssigneeIds.length > 0
          ? { assigneeIds: createAssigneeIds }
          : {}),
      };
      startCreate(createPayload);
    },
    [createAssigneeIds, defaultCreateStatusId, startCreate]
  );

  // Inline editing callbacks for kanban cards
  // When multi-select is active, apply to all selected issues
  const handleCardPriorityClick = useCallback(
    (taskId: string) => {
      const ids = isMultiSelectActive ? [...selectedTaskIds] : [taskId];
      openPrioritySelection(projectId, ids);
    },
    [projectId, openPrioritySelection, selectedTaskIds, isMultiSelectActive]
  );

  const handleCardAssigneeClick = useCallback(
    (taskId: string) => {
      const ids = isMultiSelectActive ? [...selectedTaskIds] : [taskId];
      openAssigneeSelection(projectId, ids);
    },
    [projectId, openAssigneeSelection, selectedTaskIds, isMultiSelectActive]
  );

  const handleCardMoreActionsClick = useCallback(
    (taskId: string) => {
      const ids = isMultiSelectActive ? [...selectedTaskIds] : [taskId];
      CommandBarDialog.show({
        page: 'taskActions',
        projectId,
        taskIds: ids,
      });
    },
    [projectId, selectedTaskIds, isMultiSelectActive]
  );

  const handleCardTagToggle = useCallback(
    (taskId: string, tagId: string) => {
      const currentTaskTags = getTagsForTask(taskId);
      const existing = currentTaskTags.find((it) => it.tag_id === tagId);
      if (existing) {
        removeTaskTag(existing.id);
      } else {
        insertTaskTag({ task_id: taskId, tag_id: tagId });
      }
    },
    [getTagsForTask, insertTaskTag, removeTaskTag]
  );

  const getResolvedRelationshipsForTask = useCallback(
    (taskId: string) =>
      resolveRelationshipsForTask(
        taskId,
        getRelationshipsForTask(taskId),
        tasksById
      ),
    [getRelationshipsForTask, tasksById]
  );

  const handleCreateTag = useCallback(
    (data: { name: string; color: string }): string => {
      const { data: newTag } = insertTag({
        project_id: projectId,
        name: data.name,
        color: data.color,
      });
      return newTag.id;
    },
    [insertTag, projectId]
  );

  const isLoading = projectLoading || orgLoading;

  if (isLoading) {
    return <LoadingState />;
  }

  return (
    <div className="flex flex-col h-full space-y-base">
      <div
        className={cn(
          'px-double pt-double space-y-base',
          isMobile && 'px-base pt-base'
        )}
      >
        <div
          className={cn(
            'min-w-0 items-center gap-half',
            isMobile ? 'grid grid-cols-[minmax(0,1fr)_auto]' : 'flex flex-wrap'
          )}
        >
          <h2
            className={cn(
              'text-2xl font-medium',
              isMobile && 'min-w-0 truncate text-lg'
            )}
            title={projectName}
          >
            {projectName}
          </h2>

          <ProjectWorkspaceDefaultContext
            projectId={projectId}
            hostId={routeState.hostId}
            className={cn(
              isMobile
                ? 'col-span-2 row-start-2 w-full min-w-0 max-w-full'
                : 'min-w-[12rem] max-w-full flex-1 basis-[20rem] sm:max-w-[min(560px,60vw)]'
            )}
          />

          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                className={cn(
                  'p-half rounded-sm text-low hover:text-normal hover:bg-secondary transition-colors',
                  isMobile && 'col-start-2 row-start-1'
                )}
                aria-label="Project menu"
              >
                <DotsThreeIcon className="size-icon-sm" weight="bold" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem
                onClick={() => appNavigation.goToProjectWorkflows(projectId)}
              >
                {t('workflow.templates.title')}
              </DropdownMenuItem>
              <DropdownMenuItem onClick={openProjectsGuide}>
                {t('kanban.openProjectsGuide', 'Projects guide')}
              </DropdownMenuItem>
              <DropdownMenuItem
                onClick={() => executeAction(Actions.ProjectSettings)}
              >
                {t('kanban.editProjectSettings', 'Edit project settings')}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        <div
          className={cn(
            'flex gap-base',
            isMobile
              ? 'flex-col items-start'
              : 'min-w-0 flex-nowrap items-start overflow-hidden'
          )}
        >
          <ViewNavTabs
            activeView={kanbanViewMode}
            onViewChange={setKanbanViewMode}
            hiddenStatuses={hiddenStatuses}
            selectedStatusId={listViewStatusFilter}
            onStatusSelect={setListViewStatusFilter}
            className={isMobile ? undefined : 'shrink-0'}
          />
          <KanbanFilterBar
            className={isMobile ? undefined : 'min-w-0 flex-1'}
            isFiltersDialogOpen={isFiltersDialogOpen}
            onFiltersDialogOpenChange={setIsFiltersDialogOpen}
            tags={tags}
            users={membersWithProfiles}
            activeViewId={activeViewId}
            onViewChange={handleKanbanProjectViewChange}
            viewIds={KANBAN_PROJECT_VIEW_IDS}
            projectId={projectId}
            currentUserId={userId}
            filters={kanbanFilters}
            showSubTasks={showSubTasks}
            showWorkspaces={showWorkspaces}
            hasActiveFilters={hasActiveFilters}
            onSearchQueryChange={setKanbanSearchQuery}
            onPrioritiesChange={setKanbanPriorities}
            onAssigneesChange={setKanbanAssignees}
            onTagsChange={setKanbanTags}
            onSortChange={setKanbanSort}
            onShowSubTasksChange={setShowSubTasks}
            onShowWorkspacesChange={setShowWorkspaces}
            hideBlocked={hideBlocked}
            onHideBlockedChange={setHideBlocked}
            onClearFilters={clearKanbanFilters}
            onCreateTask={handleAddTask}
            shouldAnimateCreateButton={shouldAnimateCreateButton}
            renderFiltersDialog={(props) => <KanbanFiltersDialog {...props} />}
            isMobile={isMobile}
          />
        </div>
      </div>

      {kanbanViewMode === 'kanban' ? (
        visibleStatuses.length === 0 ? (
          <div className="flex-1 flex items-center justify-center">
            <p className="text-low">{t('kanban.noVisibleStatuses')}</p>
          </div>
        ) : (
          <div className="flex-1 overflow-x-auto px-double">
            <KanbanProvider
              onDragEnd={handleDragEnd}
              className="min-w-full"
              columnClassName="auto-cols-[minmax(260px,1fr)]"
            >
              {visibleStatuses.map((status) => {
                const taskIds = items[status.id] ?? [];

                return (
                  <KanbanBoard key={status.id}>
                    <KanbanHeader>
                      <div className="border-t sticky border-b top-0 z-20 flex shrink-0 items-center justify-between gap-2 p-base bg-secondary">
                        <div className="flex items-center gap-2">
                          <div
                            className="h-2 w-2 rounded-full shrink-0"
                            style={{ backgroundColor: `hsl(${status.color})` }}
                          />
                          <p className="m-0 text-sm">{status.name}</p>
                        </div>
                        <button
                          type="button"
                          onClick={() => handleAddTask(status.id)}
                          className="p-half rounded-sm text-low hover:text-normal hover:bg-secondary transition-colors"
                          aria-label="Add task"
                        >
                          <PlusIcon className="size-icon-xs" weight="bold" />
                        </button>
                      </div>
                    </KanbanHeader>
                    <KanbanCards id={status.id}>
                      {taskIds.map((taskId, index) => {
                        const task = taskMap[taskId];
                        if (!task) return null;
                        const taskWorkflowAttempts =
                          workflowAttemptsByTaskId.get(task.id) ?? [];
                        const taskWorkspaces =
                          workspacesByTaskId.get(task.id) ?? [];
                        const workspaceIdsShownOnCard = new Set(
                          taskWorkspaces.map((workspace) => workspace.id)
                        );
                        const taskCardPullRequests = getPullRequestsForTask(
                          task.id
                        ).filter((pr) => {
                          if (!pr.workspace_id) {
                            return true;
                          }

                          // If this PR is already visible under a workspace card,
                          // do not render it again at the issue level.
                          return !workspaceIdsShownOnCard.has(pr.workspace_id);
                        });

                        return (
                          <KanbanCard
                            key={task.id}
                            id={task.id}
                            name={task.title}
                            index={index}
                            className="group"
                            onClick={(e) => handleCardClick(task.id, e)}
                            isOpen={selectedKanbanTaskId === task.id}
                            isMobile={isMobile}
                            isSelected={selectedTaskIds.has(task.id)}
                            dragDisabled={isMultiSelectActive}
                          >
                            <KanbanCardContent
                              displayId={task.simple_id}
                              title={task.title}
                              description={task.description}
                              priority={task.priority}
                              tags={getTagObjectsForTask(task.id)}
                              assignees={taskAssigneesMap[task.id] ?? []}
                              pullRequests={taskCardPullRequests}
                              relationships={resolveRelationshipsForTask(
                                task.id,
                                getRelationshipsForTask(task.id),
                                tasksById
                              )}
                              isSubTask={!!task.parent_task_id}
                              isMobile={isMobile}
                              onPriorityClick={(e) => {
                                e.stopPropagation();
                                handleCardPriorityClick(task.id);
                              }}
                              onAssigneeClick={(e) => {
                                e.stopPropagation();
                                handleCardAssigneeClick(task.id);
                              }}
                              onMoreActionsClick={() =>
                                handleCardMoreActionsClick(task.id)
                              }
                              tagEditProps={{
                                allTags: tags,
                                selectedTagIds: getTagsForTask(task.id).map(
                                  (it) => it.tag_id
                                ),
                                onTagToggle: (tagId) =>
                                  handleCardTagToggle(task.id, tagId),
                                onCreateTag: handleCreateTag,
                                renderTagEditor: ({
                                  allTags,
                                  selectedTagIds,
                                  onTagToggle,
                                  onCreateTag,
                                  trigger,
                                }) => (
                                  <SearchableTagDropdownContainer
                                    tags={allTags}
                                    selectedTagIds={selectedTagIds}
                                    onTagToggle={onTagToggle}
                                    onCreateTag={onCreateTag}
                                    disabled={false}
                                    contentClassName=""
                                    trigger={trigger}
                                  />
                                ),
                              }}
                            />
                            {(taskWorkflowAttempts.length > 0 ||
                              taskWorkspaces.length > 0) && (
                              <div className="mt-base flex flex-col gap-half">
                                {taskWorkflowAttempts.map((attempt) => (
                                  <TaskWorkflowAttemptCard
                                    key={attempt.id}
                                    attempt={workflowAttemptToKanbanCard(
                                      attempt
                                    )}
                                    onClick={() =>
                                      openTaskWorkflowAttempt(
                                        attempt.workflow_id
                                      )
                                    }
                                  />
                                ))}
                                {taskWorkspaces.map((workspace) => (
                                  <TaskWorkspaceCard
                                    key={workspace.id}
                                    workspace={workspace}
                                    onClick={
                                      workspace.localWorkspaceId
                                        ? () =>
                                            openTaskWorkspace(
                                              task.id,
                                              workspace.localWorkspaceId!
                                            )
                                        : undefined
                                    }
                                    showOwner={false}
                                    showStatusBadge={false}
                                    showNoPrText={false}
                                  />
                                ))}
                              </div>
                            )}
                          </KanbanCard>
                        );
                      })}
                    </KanbanCards>
                  </KanbanBoard>
                );
              })}
            </KanbanProvider>
          </div>
        )
      ) : (
        <div className="flex-1 overflow-y-auto px-double">
          <KanbanProvider onDragEnd={handleDragEnd} className="!block !w-full">
            <TaskListView
              statuses={listViewStatuses}
              items={items}
              taskMap={taskMap}
              taskAssigneesMap={taskAssigneesMap}
              getTagObjectsForTask={getTagObjectsForTask}
              getResolvedRelationshipsForTask={getResolvedRelationshipsForTask}
              onTaskClick={handleCardClick}
              selectedTaskId={selectedKanbanTaskId}
              selectedTaskIds={selectedTaskIds}
              isMultiSelectActive={isMultiSelectActive}
              onTaskCheckboxChange={handleCheckboxChange}
            />
          </KanbanProvider>
        </div>
      )}

      {isMultiSelectActive && <BulkActionBarContainer projectId={projectId} />}
    </div>
  );
}
