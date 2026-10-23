import { useEffect, useState, useMemo, useCallback, useRef } from 'react';
import { createPortal, flushSync } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import {
  DragDropContext,
  Droppable,
  Draggable,
  type DropResult,
  type DraggableProvided,
  type DraggableStateSnapshot,
  type DroppableProvided,
  type DraggableRubric,
} from '@hello-pangea/dnd';
import {
  SpinnerIcon,
  PlusIcon,
  SignInIcon,
  XIcon,
  DotsSixVerticalIcon,
  PencilSimpleLineIcon,
  CheckIcon,
  CaretDownIcon,
  SlidersHorizontalIcon,
  WarningCircleIcon,
} from '@phosphor-icons/react';
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from '@vibe/ui/components/Accordion';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@vibe/ui/components/Dropdown';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@vibe/ui/components/Popover';
import { PrimaryButton } from '@vibe/ui/components/PrimaryButton';
import { Switch } from '@vibe/ui/components/Switch';
import { useUserOrganizations } from '@/shared/hooks/useUserOrganizations';
import { useAuth } from '@/shared/hooks/auth/useAuth';
import { OAuthDialog } from '@/shared/dialogs/global/OAuthDialog';
import { CreateRemoteProjectDialog } from '@/shared/dialogs/org/CreateRemoteProjectDialog';
import { ProjectActionsMenu } from '@/shared/components/ProjectActionsMenu';
import { isDefaultProject } from '@/shared/lib/defaultProject';
import { formatWorkspacePathForDisplay } from '@/shared/lib/workspaceContext';
import { useDeleteProject } from '@/shared/hooks/useDeleteProject';
import { useShape } from '@/shared/integrations/electric/hooks';
import { bulkUpdateProjectStatuses } from '@/shared/lib/remoteApi';

import {
  PROJECTS_SHAPE,
  PROJECT_MUTATION,
  PROJECT_PROJECT_STATUSES_SHAPE,
  PROJECT_STATUS_MUTATION,
  PROJECT_TASKS_SHAPE,
} from 'shared/remote-types';
import { getRandomPresetColor, PRESET_COLORS } from '@/shared/lib/colors';
import { InlineColorPicker } from '@vibe/ui/components/ColorPicker';
import { cn } from '@/shared/lib/utils';
import {
  SettingsCard,
  SettingsField,
  SettingsInput,
  SettingsSaveBar,
  TwoColumnPicker,
  TwoColumnPickerColumn,
  TwoColumnPickerItem,
  TwoColumnPickerBadge,
  TwoColumnPickerEmpty,
} from './SettingsComponents';
import { useSettingsDirty } from './SettingsDirtyContext';
import { ReposSettingsSection } from './ReposSettingsSection';
import {
  SettingsMachineUserSystemProvider,
  useSettingsMachineState,
} from './SettingsMachineUserSystemProvider';
import type { GitBranch, Repo } from 'shared/types';
import { WorkspaceTargetDialog } from '@/shared/dialogs/shared/WorkspaceTargetDialog';
import {
  areProjectWorkspaceDefaultsEqual,
  getProjectWorkspaceDefaultOrThrow,
  projectWorkspaceDefaultQueryKey,
  saveProjectWorkspaceDefault,
  type ProjectWorkspaceDefault,
} from '@/shared/hooks/useProjectRepoDefaults';
import {
  useSettingsHost,
  useSettingsMachineClient,
} from './SettingsHostContext';

interface FormState {
  name: string;
  color: string;
}

interface RemoteProjectsSettingsSectionProps {
  initialState?: { organizationId?: string; projectId?: string };
  scoped?: boolean;
}

interface StatusItem {
  id: string;
  name: string;
  color: string;
  hidden: boolean;
  sort_order: number;
  isNew: boolean;
}

interface StatusRowCloneProps {
  status: StatusItem;
  provided: DraggableProvided;
}

function StatusRowClone({ status, provided }: StatusRowCloneProps) {
  return createPortal(
    <div
      ref={provided.innerRef}
      {...provided.draggableProps}
      {...provided.dragHandleProps}
      className={cn(
        'flex items-center gap-base px-base py-half rounded-sm shadow-lg',
        status.isNew ? 'bg-panel' : 'bg-secondary',
        status.hidden && 'opacity-50'
      )}
      style={{
        ...provided.draggableProps.style,
        zIndex: 10001,
      }}
    >
      <div className="flex items-center justify-center size-icon-sm cursor-grabbing">
        <DotsSixVerticalIcon className="size-icon-xs text-low" weight="bold" />
      </div>
      <div
        className="size-dot rounded-full shrink-0"
        style={{ backgroundColor: `hsl(${status.color})` }}
      />
      <span className="text-sm text-high">{status.name}</span>
    </div>,
    document.body
  );
}

interface StatusRowProps {
  status: StatusItem;
  index: number;
  taskCount: number;
  visibleCount: number;
  editingId: string | null;
  editingColorId: string | null;
  onToggleHidden: (id: string, hidden: boolean) => void;
  onNameChange: (id: string, name: string) => void;
  onColorChange: (id: string, color: string) => void;
  onDelete: (id: string) => void;
  onStartEditing: (id: string) => void;
  onStartEditingColor: (id: string | null) => void;
  onStopEditing: () => void;
}

function StatusRow({
  status,
  index,
  taskCount,
  visibleCount,
  editingId,
  editingColorId,
  onToggleHidden,
  onNameChange,
  onColorChange,
  onDelete,
  onStartEditing,
  onStartEditingColor,
  onStopEditing,
}: StatusRowProps) {
  const { t } = useTranslation('common');
  const [localName, setLocalName] = useState(status.name);
  const isEditing = editingId === status.id;
  const isEditingColor = editingColorId === status.id;
  const isLastVisible = !status.hidden && visibleCount === 1;
  const canDelete = taskCount === 0;

  useEffect(() => {
    setLocalName(status.name);
  }, [status.name]);

  const handleNameKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      if (localName.trim()) {
        onNameChange(status.id, localName.trim());
      } else {
        setLocalName(status.name);
      }
      onStopEditing();
    } else if (e.key === 'Escape') {
      setLocalName(status.name);
      onStopEditing();
    }
  };

  const handleNameBlur = () => {
    if (localName.trim() && localName !== status.name) {
      onNameChange(status.id, localName.trim());
    } else {
      setLocalName(status.name);
    }
    onStopEditing();
  };

  return (
    <Draggable draggableId={status.id} index={index}>
      {(provided: DraggableProvided, snapshot: DraggableStateSnapshot) => (
        <div
          ref={provided.innerRef}
          {...provided.draggableProps}
          className={cn(
            'flex items-center justify-between px-base py-half rounded-sm',
            status.isNew ? 'bg-panel' : 'bg-secondary',
            status.hidden && 'opacity-50',
            snapshot.isDragging && 'shadow-lg opacity-80'
          )}
          style={{
            ...provided.draggableProps.style,
            zIndex: snapshot.isDragging ? 10 : undefined,
          }}
        >
          <div className="flex items-center gap-base">
            <div
              {...provided.dragHandleProps}
              className="flex items-center justify-center size-icon-sm cursor-grab"
            >
              <DotsSixVerticalIcon
                className="size-icon-xs text-low"
                weight="bold"
              />
            </div>

            <Popover
              open={isEditingColor}
              onOpenChange={(open) =>
                onStartEditingColor(open ? status.id : null)
              }
            >
              <PopoverTrigger asChild>
                <button
                  type="button"
                  className="flex items-center justify-center size-icon-sm"
                  title={t('kanban.changeColor', 'Change color')}
                >
                  <div
                    className="size-dot rounded-full shrink-0"
                    style={{ backgroundColor: `hsl(${status.color})` }}
                  />
                </button>
              </PopoverTrigger>
              <PopoverContent
                align="start"
                className="w-auto p-base"
                onInteractOutside={(e) => {
                  e.preventDefault();
                  onStartEditingColor(null);
                }}
              >
                <InlineColorPicker
                  value={status.color}
                  onChange={(color) => onColorChange(status.id, color)}
                  colors={PRESET_COLORS}
                />
              </PopoverContent>
            </Popover>

            {isEditing ? (
              <input
                type="text"
                value={localName}
                onChange={(e) => setLocalName(e.target.value)}
                onKeyDown={handleNameKeyDown}
                onBlur={handleNameBlur}
                autoFocus
                className="bg-transparent text-sm text-high outline-none border-b border-brand w-24"
              />
            ) : (
              <span
                className="text-sm text-high cursor-pointer"
                onClick={() => onStartEditing(status.id)}
              >
                {status.name}
              </span>
            )}
          </div>

          <div className="flex items-center gap-base">
            <button
              type="button"
              onClick={() => onStartEditing(status.id)}
              className="flex items-center justify-center size-icon-sm text-low hover:text-normal"
              title={t('kanban.editName', 'Edit name')}
            >
              <PencilSimpleLineIcon className="size-icon-xs" weight="bold" />
            </button>
            <button
              type="button"
              onClick={() => canDelete && onDelete(status.id)}
              className={cn(
                'flex items-center justify-center size-icon-sm',
                canDelete
                  ? 'text-low hover:text-normal'
                  : 'text-low opacity-50 cursor-not-allowed'
              )}
              title={
                canDelete
                  ? t('kanban.deleteStatus', 'Delete status')
                  : t('kanban.cannotDeleteWithIssues', 'Move tasks first')
              }
              disabled={!canDelete}
            >
              <XIcon className="size-icon-xs" weight="bold" />
            </button>
            <Switch
              checked={!status.hidden}
              onCheckedChange={(checked) => onToggleHidden(status.id, !checked)}
              disabled={isLastVisible && !status.hidden}
              title={
                isLastVisible
                  ? t(
                      'kanban.lastVisibleStatus',
                      'At least one status must be visible'
                    )
                  : status.hidden
                    ? t('kanban.showStatus', 'Show status')
                    : t('kanban.hideStatus', 'Hide status')
              }
            />
          </div>
        </div>
      )}
    </Draggable>
  );
}

export function RemoteProjectsSettingsSection({
  initialState,
  scoped = false,
}: RemoteProjectsSettingsSectionProps) {
  const { t } = useTranslation(['settings', 'common', 'projects']);
  const navigate = useNavigate();
  const { setDirty: setContextDirty } = useSettingsDirty();
  const { selectedHost, selectedHostId } = useSettingsHost();
  const { isSignedIn, isLoaded } = useAuth();
  const queryClient = useQueryClient();
  const machineClient = useSettingsMachineClient();
  const machineState = useSettingsMachineState();
  const machineScopeKey = machineClient?.queryScopeKey.join(':') ?? 'none';
  const repoDefaultsHostId = machineClient?.target.apiHostId;

  // Selection state - initialize with provided values
  const [selectedOrgId, setSelectedOrgId] = useState<string | null>(
    initialState?.organizationId ?? null
  );
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(
    initialState?.projectId ?? null
  );
  const ownerRef = useRef({
    projectId: selectedProjectId,
    machineScopeKey,
    canMutate: machineState.canMutate,
  });
  ownerRef.current = {
    projectId: selectedProjectId,
    machineScopeKey,
    canMutate: machineState.canMutate,
  };
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Form state for editing
  const [formState, setFormState] = useState<FormState | null>(null);

  // UI state
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);

  // Workspace default state for the selected project on this host.
  const [workspaceDefault, setWorkspaceDefault] =
    useState<ProjectWorkspaceDefault | null>(null);
  const [savedWorkspaceDefault, setSavedWorkspaceDefault] =
    useState<ProjectWorkspaceDefault | null>(null);
  const [isLoadingDefaults, setIsLoadingDefaults] = useState(false);
  const [allRepos, setAllRepos] = useState<Repo[]>([]);
  const [defaultReposError, setDefaultReposError] = useState<string | null>(
    null
  );
  const [branchCache, setBranchCache] = useState<Map<string, GitBranch[]>>(
    new Map()
  );
  const [loadingBranches, setLoadingBranches] = useState<Set<string>>(
    new Set()
  );
  const [repoConfigDirty, setRepoConfigDirty] = useState(false);
  const workspaceDraftDirtyRef = useRef(false);

  // Fetch organizations
  const {
    data: orgsResponse,
    isLoading: orgsLoading,
    error: orgsError,
  } = useUserOrganizations({ hostId: selectedHost?.apiHostId ?? null });

  const organizations = useMemo(
    () => orgsResponse?.organizations ?? [],
    [orgsResponse?.organizations]
  );

  // Auto-select first org when loaded (only if no initial org was provided)
  useEffect(() => {
    if (
      !initialState?.organizationId &&
      organizations.length > 0 &&
      !selectedOrgId
    ) {
      setSelectedOrgId(organizations[0].id);
    }
  }, [organizations, selectedOrgId, initialState?.organizationId]);

  // Fetch projects for selected org
  const params = useMemo(
    () => ({ organization_id: selectedOrgId || '' }),
    [selectedOrgId]
  );

  const {
    data: projects,
    isLoading: projectsLoading,
    error: projectsError,
    retry: retryProjects,
    update,
  } = useShape(PROJECTS_SHAPE, params, {
    hostId: selectedHost?.apiHostId ?? null,
    enabled: !!selectedOrgId,
    mutation: PROJECT_MUTATION,
  });

  // Initialize form state when project is pre-selected and projects are loaded
  useEffect(() => {
    if (initialState?.projectId && projects.length > 0 && !formState) {
      const project = projects.find((p) => p.id === initialState.projectId);
      if (project) {
        setFormState({ name: project.name, color: project.color });
      }
    }
  }, [initialState?.projectId, projects, formState]);

  // Find selected project
  const selectedProject = useMemo(
    () => projects.find((p) => p.id === selectedProjectId) ?? null,
    [projects, selectedProjectId]
  );
  const { deleteProject: handleDeleteProject, pendingProjectId } =
    useDeleteProject({
      hostId: selectedHost?.apiHostId ?? null,
      scopeKey: `settings:${selectedOrgId}:${selectedProjectId}:${selectedHostId}`,
      enabled: isSignedIn && !isSaving && !projectsLoading && !projectsError,
      onDeleted: (project) => {
        if (selectedProjectId === project.id) {
          // The named deletion confirmation discards this project's draft only
          // after persistence. Update the route blocker before leaving the editor.
          flushSync(() => {
            setSelectedProjectId(null);
            setFormState(null);
            setHasStatusChanges(false);
            setRepoConfigDirty(false);
            setContextDirty('remote-projects', false);
          });
        }
        if (scoped)
          void navigate({
            to: '/projects',
            search: { host_id: selectedHost?.apiHostId ?? undefined },
            replace: true,
          });
      },
    });

  // Fetch statuses and issues for selected project (for status settings)
  const projectParams = useMemo(
    () => ({ project_id: selectedProjectId ?? '' }),
    [selectedProjectId]
  );

  const {
    data: projectStatuses,
    insert: insertProjectStatus,
    update: updateProjectStatus,
    remove: removeProjectStatus,
  } = useShape(PROJECT_PROJECT_STATUSES_SHAPE, projectParams, {
    hostId: selectedHost?.apiHostId ?? null,
    enabled: !!selectedProjectId && !isDefaultProject(selectedProjectId),
    mutation: PROJECT_STATUS_MUTATION,
  });

  const { data: projectTasks } = useShape(PROJECT_TASKS_SHAPE, projectParams, {
    enabled: !!selectedProjectId && !isDefaultProject(selectedProjectId),
    hostId: selectedHost?.apiHostId ?? null,
  });

  const taskCountByStatus = useMemo(() => {
    const counts: Record<string, number> = {};

    for (const status of projectStatuses) {
      counts[status.id] = 0;
    }

    for (const task of projectTasks) {
      counts[task.status_id] = (counts[task.status_id] ?? 0) + 1;
    }

    return counts;
  }, [projectStatuses, projectTasks]);

  const sortedProjectStatuses = useMemo(
    () => [...projectStatuses].sort((a, b) => a.sort_order - b.sort_order),
    [projectStatuses]
  );

  const [localStatuses, setLocalStatuses] = useState<StatusItem[]>([]);
  const [editingStatusId, setEditingStatusId] = useState<string | null>(null);
  const [editingStatusColorId, setEditingStatusColorId] = useState<
    string | null
  >(null);
  const [hasStatusChanges, setHasStatusChanges] = useState(false);

  useEffect(() => {
    if (!selectedProjectId) {
      setLocalStatuses([]);
      setHasStatusChanges(false);
      return;
    }

    if (!hasStatusChanges) {
      setLocalStatuses(
        sortedProjectStatuses.map((status) => ({
          id: status.id,
          name: status.name,
          color: status.color,
          hidden: status.hidden,
          sort_order: status.sort_order,
          isNew: false,
        }))
      );
    }
  }, [selectedProjectId, sortedProjectStatuses, hasStatusChanges]);

  // Load default repos and registered repos when project changes
  useEffect(() => {
    if (!selectedProjectId || isDefaultProject(selectedProjectId)) {
      setWorkspaceDefault(null);
      setSavedWorkspaceDefault(null);
      setAllRepos([]);
      setDefaultReposError(null);
      setIsLoadingDefaults(false);
      return;
    }
    if (!machineState.canMutate) return;
    let cancelled = false;
    setIsLoadingDefaults(true);
    setDefaultReposError(null);
    setBranchCache(new Map());
    setLoadingBranches(new Set());

    Promise.all([
      getProjectWorkspaceDefaultOrThrow(selectedProjectId, repoDefaultsHostId),
      machineClient?.listRepos().catch(() => {
        setDefaultReposError(
          t('settings:settings.remoteProjects.form.defaultRepos.fetchError')
        );
        return [] as Repo[];
      }) ?? Promise.resolve([] as Repo[]),
    ])
      .then(([projectWorkspaceDefault, registeredRepos]) => {
        if (cancelled) return;
        if (!workspaceDraftDirtyRef.current) {
          setWorkspaceDefault(projectWorkspaceDefault);
          setSavedWorkspaceDefault(projectWorkspaceDefault);
        }
        setAllRepos(registeredRepos);
      })
      .catch(() => {
        if (cancelled) return;
        setWorkspaceDefault(null);
        setSavedWorkspaceDefault(null);
        setDefaultReposError(
          t('settings:settings.remoteProjects.form.defaultRepos.fetchError')
        );
      })
      .finally(() => {
        if (!cancelled) setIsLoadingDefaults(false);
      });
    return () => {
      cancelled = true;
    };
  }, [
    machineClient,
    machineScopeKey,
    machineState.canMutate,
    repoDefaultsHostId,
    selectedProjectId,
    t,
  ]);

  const handleChooseProjectWorkspace = useCallback(async () => {
    if (!selectedProjectId || !ownerRef.current.canMutate) return;
    const owner = ownerRef.current;

    const currentRepo =
      workspaceDefault?.kind === 'git'
        ? allRepos.find((repo) => repo.id === workspaceDefault.repo.repo_id)
        : undefined;
    setDefaultReposError(null);

    try {
      const result = await WorkspaceTargetDialog.show({
        initialPath:
          workspaceDefault?.kind === 'direct_folder'
            ? workspaceDefault.path
            : currentRepo?.path,
        initialMode:
          workspaceDefault?.kind === 'direct_folder'
            ? 'direct_folder'
            : 'worktree',
        initialBranch:
          workspaceDefault?.kind === 'git'
            ? workspaceDefault.repo.target_branch
            : null,
        hostId: repoDefaultsHostId,
        purpose: 'project_default',
        title: t('settings:settings.remoteProjects.form.defaultRepos.addNew'),
        description: t(
          'settings:settings.remoteProjects.form.defaultRepos.pickerDescription',
          {
            host: selectedHost?.label ?? t('settings:hostPicker.thisMachine'),
          }
        ),
      });
      if (
        result.kind !== 'confirmed' ||
        !mountedRef.current ||
        !ownerRef.current.canMutate ||
        owner.projectId !== ownerRef.current.projectId ||
        owner.machineScopeKey !== ownerRef.current.machineScopeKey
      )
        return;

      const nextWorkspaceDefault: ProjectWorkspaceDefault =
        result.selection.mode === 'worktree'
          ? {
              kind: 'git',
              repo: {
                repo_id: result.selection.repo.id,
                target_branch: result.selection.targetBranch,
              },
            }
          : {
              kind: 'direct_folder',
              path: result.selection.path,
            };
      const nextRepoId =
        nextWorkspaceDefault.kind === 'git'
          ? nextWorkspaceDefault.repo.repo_id
          : null;
      const currentRepoId =
        workspaceDefault?.kind === 'git' ? workspaceDefault.repo.repo_id : null;
      if (
        repoConfigDirty &&
        currentRepoId !== nextRepoId &&
        !window.confirm(
          t('settings.common.discardChangesConfirm', 'Discard unsaved changes?')
        )
      ) {
        return;
      }

      if (result.selection.mode === 'worktree') {
        const nextRepo = result.selection.repo;
        setAllRepos((previous) => {
          const exists = previous.some((repo) => repo.id === nextRepo.id);
          return exists
            ? previous.map((repo) =>
                repo.id === nextRepo.id ? nextRepo : repo
              )
            : [...previous, nextRepo];
        });
      }
      setWorkspaceDefault(nextWorkspaceDefault);
    } catch {
      setDefaultReposError(
        t('settings:settings.remoteProjects.form.defaultRepos.saveError')
      );
    }
  }, [
    allRepos,
    workspaceDefault,
    t,
    repoConfigDirty,
    repoDefaultsHostId,
    selectedHost?.label,
    selectedProjectId,
  ]);

  const handleRemoveProjectWorkspace = useCallback(() => {
    if (!selectedProjectId) return;
    if (
      repoConfigDirty &&
      !window.confirm(
        t('settings.common.discardChangesConfirm', 'Discard unsaved changes?')
      )
    ) {
      return;
    }
    setDefaultReposError(null);
    setWorkspaceDefault(null);
  }, [selectedProjectId, repoConfigDirty, t]);

  const fetchBranchesForRepo = useCallback(
    async (repoId: string) => {
      if (branchCache.has(repoId)) return;
      setLoadingBranches((prev) => new Set(prev).add(repoId));
      try {
        if (!machineClient) return;
        const branches = await machineClient.getRepoBranches(repoId);
        setBranchCache((prev) => new Map(prev).set(repoId, branches));
      } catch {
        setDefaultReposError(
          t('settings:settings.remoteProjects.form.defaultRepos.fetchError')
        );
      } finally {
        setLoadingBranches((prev) => {
          const next = new Set(prev);
          next.delete(repoId);
          return next;
        });
      }
    },
    [branchCache, machineClient, t]
  );

  const handleChangeBranch = useCallback(
    (repoId: string, newBranch: string) => {
      if (!selectedProjectId) return;
      if (
        workspaceDefault?.kind !== 'git' ||
        workspaceDefault.repo.repo_id !== repoId
      ) {
        return;
      }
      setDefaultReposError(null);
      setWorkspaceDefault({
        kind: 'git',
        repo: { ...workspaceDefault.repo, target_branch: newBranch },
      });
    },
    [selectedProjectId, workspaceDefault]
  );

  const visibleStatusCount = useMemo(
    () => localStatuses.filter((status) => !status.hidden).length,
    [localStatuses]
  );

  const isProjectDirty = useMemo(() => {
    if (!selectedProject || !formState) return false;
    return (
      formState.name !== selectedProject.name ||
      formState.color !== selectedProject.color
    );
  }, [selectedProject, formState]);

  const hasDefaultRepoChanges = useMemo(
    () =>
      !areProjectWorkspaceDefaultsEqual(
        workspaceDefault,
        savedWorkspaceDefault
      ),
    [savedWorkspaceDefault, workspaceDefault]
  );
  workspaceDraftDirtyRef.current = hasDefaultRepoChanges;
  const gitWorkspaceDefault =
    workspaceDefault?.kind === 'git' ? workspaceDefault.repo : null;
  const workspaceDefaultRepo = gitWorkspaceDefault
    ? allRepos.find((repo) => repo.id === gitWorkspaceDefault.repo_id)
    : undefined;
  const workspaceDirectoryPath =
    workspaceDefault?.kind === 'direct_folder'
      ? workspaceDefault.path
      : workspaceDefaultRepo?.path;
  const workspaceDirectoryLabel = workspaceDirectoryPath
    ? formatWorkspacePathForDisplay(workspaceDirectoryPath)
    : undefined;

  const isDirty = isProjectDirty || hasStatusChanges || hasDefaultRepoChanges;

  // Sync dirty state to context for unsaved changes confirmation
  useEffect(() => {
    setContextDirty('remote-projects', isDirty || repoConfigDirty);
    return () => setContextDirty('remote-projects', false);
  }, [isDirty, repoConfigDirty, setContextDirty]);

  const handleStatusToggleHidden = useCallback(
    (id: string, hidden: boolean) => {
      setLocalStatuses((prev) =>
        prev.map((status) =>
          status.id === id ? { ...status, hidden } : status
        )
      );
      setHasStatusChanges(true);
    },
    []
  );

  const handleStatusNameChange = useCallback((id: string, name: string) => {
    setLocalStatuses((prev) =>
      prev.map((status) => (status.id === id ? { ...status, name } : status))
    );
    setHasStatusChanges(true);
  }, []);

  const handleStatusColorChange = useCallback((id: string, color: string) => {
    setLocalStatuses((prev) =>
      prev.map((status) => (status.id === id ? { ...status, color } : status))
    );
    setHasStatusChanges(true);
  }, []);

  const handleStatusDelete = useCallback((id: string) => {
    setLocalStatuses((prev) => prev.filter((status) => status.id !== id));
    setHasStatusChanges(true);
  }, []);

  const handleStatusAdd = useCallback(() => {
    const newId = crypto.randomUUID();
    const maxSortOrder = localStatuses.reduce(
      (max, status) => Math.max(max, status.sort_order),
      0
    );

    setLocalStatuses((prev) => [
      ...prev,
      {
        id: newId,
        name: t('kanban.newStatus', 'New Status'),
        color: getRandomPresetColor(),
        hidden: false,
        sort_order: maxSortOrder + 1000,
        isNew: true,
      },
    ]);
    setEditingStatusId(newId);
    setHasStatusChanges(true);
  }, [localStatuses, t]);

  const handleStatusDragEnd = useCallback((result: DropResult) => {
    const { source, destination } = result;
    if (!destination || source.index === destination.index) return;

    setLocalStatuses((prev) => {
      const reordered = [...prev];
      const [moved] = reordered.splice(source.index, 1);
      reordered.splice(destination.index, 0, moved);
      return reordered.map((status, index) => ({
        ...status,
        sort_order: index,
      }));
    });
    setHasStatusChanges(true);
  }, []);

  const persistStatusChanges = useCallback(async () => {
    const originalById = new Map(
      projectStatuses.map((status) => [status.id, status])
    );
    const mutationPromises: Promise<unknown>[] = [];
    const localIds = new Set(localStatuses.map((status) => status.id));

    for (const original of projectStatuses) {
      if (!localIds.has(original.id)) {
        const result = removeProjectStatus(original.id);
        mutationPromises.push(result.persisted);
      }
    }

    const bulkUpdates: {
      id: string;
      changes: Partial<{
        name: string;
        color: string;
        sort_order: number;
        hidden: boolean;
      }>;
    }[] = [];

    for (const local of localStatuses) {
      const original = originalById.get(local.id);

      if (!original) {
        const result = insertProjectStatus({
          id: local.id,
          project_id: selectedProjectId ?? '',
          name: local.name,
          color: local.color,
          sort_order: local.sort_order,
          hidden: local.hidden,
        });
        mutationPromises.push(result.persisted);
        continue;
      }

      const changes: Partial<{
        name: string;
        color: string;
        sort_order: number;
        hidden: boolean;
      }> = {
        sort_order: local.sort_order,
      };

      if (local.name !== original.name) changes.name = local.name;
      if (local.color !== original.color) changes.color = local.color;
      if (local.hidden !== original.hidden) changes.hidden = local.hidden;

      bulkUpdates.push({ id: local.id, changes });
    }

    if (bulkUpdates.length > 1) {
      await bulkUpdateProjectStatuses(
        bulkUpdates,
        selectedHost?.apiHostId ?? null
      );
    } else if (bulkUpdates.length === 1) {
      const result = updateProjectStatus(
        bulkUpdates[0].id,
        bulkUpdates[0].changes
      );
      mutationPromises.push(result.persisted);
    }

    await Promise.all(mutationPromises);
  }, [
    projectStatuses,
    localStatuses,
    selectedProjectId,
    selectedHost?.apiHostId,
    removeProjectStatus,
    insertProjectStatus,
    updateProjectStatus,
  ]);

  // Handlers
  const handleOrgSelect = (orgId: string) => {
    if (orgId === selectedOrgId) return;
    if (isDirty || repoConfigDirty) {
      const confirmed = window.confirm(
        t('settings.common.discardChangesConfirm', 'Discard unsaved changes?')
      );
      if (!confirmed) return;
    }
    setSelectedOrgId(orgId);
    setSelectedProjectId(null);
    setFormState(null);
    setLocalStatuses([]);
    setHasStatusChanges(false);
    setEditingStatusId(null);
    setEditingStatusColorId(null);
    setError(null);
    setSuccess(null);
  };

  const handleProjectSelect = (projectId: string) => {
    if (projectId === selectedProjectId) return;
    if (isDirty || repoConfigDirty) {
      const confirmed = window.confirm(
        t('settings.common.discardChangesConfirm', 'Discard unsaved changes?')
      );
      if (!confirmed) return;
    }
    const project = projects.find((p) => p.id === projectId);
    setSelectedProjectId(projectId);
    setFormState(project ? { name: project.name, color: project.color } : null);
    setHasStatusChanges(false);
    setEditingStatusId(null);
    setEditingStatusColorId(null);
    setError(null);
    setSuccess(null);
  };

  const handleCreateProject = async () => {
    if (!selectedOrgId) return;

    try {
      const result = await CreateRemoteProjectDialog.show({
        organizationId: selectedOrgId,
        initialHostId: selectedHostId ?? undefined,
      });

      if (result.action === 'created' && result.project) {
        setSelectedProjectId(result.project.id);
        setFormState({
          name: result.project.name,
          color: result.project.color,
        });
        setSuccess(
          t(
            'settings.remoteProjects.createSuccess',
            'Project created successfully'
          )
        );
        setTimeout(() => setSuccess(null), 3000);
      }
    } catch {
      // Dialog cancelled
    }
  };

  const handleSave = async () => {
    if (isDefaultProject(selectedProjectId)) return;
    if (!selectedProjectId || !formState) return;
    if (hasDefaultRepoChanges && !ownerRef.current.canMutate) return;

    const trimmedName = formState.name.trim();
    if (!trimmedName) {
      setError(
        t('settings.remoteProjects.nameRequired', 'Project name is required')
      );
      return;
    }

    setError(null);
    setDefaultReposError(null);
    setIsSaving(true);

    try {
      if (isProjectDirty) {
        const result = update(selectedProjectId, {
          name: trimmedName,
          color: formState.color,
        });
        await result.persisted;
        await queryClient.invalidateQueries({
          predicate: (query) =>
            query.queryKey[0] === 'app-shell' ||
            (query.queryKey[0] === 'project-settings' &&
              query.queryKey[1] === selectedProjectId),
        });
      }

      if (hasStatusChanges) {
        await persistStatusChanges();
        setLocalStatuses((prev) =>
          prev.map((status) => ({ ...status, isNew: false }))
        );
        setHasStatusChanges(false);
      }

      if (hasDefaultRepoChanges) {
        if (
          !mountedRef.current ||
          !ownerRef.current.canMutate ||
          ownerRef.current.projectId !== selectedProjectId ||
          ownerRef.current.machineScopeKey !== machineScopeKey
        )
          return;
        try {
          await saveProjectWorkspaceDefault(
            selectedProjectId,
            workspaceDefault,
            repoDefaultsHostId
          );
        } catch {
          setDefaultReposError(
            t('settings:settings.remoteProjects.form.defaultRepos.saveError')
          );
          return;
        }

        setSavedWorkspaceDefault(workspaceDefault);
        await queryClient.invalidateQueries({
          queryKey: projectWorkspaceDefaultQueryKey(
            selectedProjectId,
            repoDefaultsHostId
          ),
        });
      }

      setSuccess(
        t('settings.remoteProjects.saveSuccess', 'Project updated successfully')
      );
      setTimeout(() => setSuccess(null), 3000);
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : t('settings.remoteProjects.saveError', 'Failed to update project')
      );
    } finally {
      setIsSaving(false);
    }
  };

  const handleDiscard = () => {
    if (selectedProject) {
      setFormState({
        name: selectedProject.name,
        color: selectedProject.color,
      });
    }
    setLocalStatuses(
      sortedProjectStatuses.map((status) => ({
        id: status.id,
        name: status.name,
        color: status.color,
        hidden: status.hidden,
        sort_order: status.sort_order,
        isNew: false,
      }))
    );
    setHasStatusChanges(false);
    setWorkspaceDefault(savedWorkspaceDefault);
    setDefaultReposError(null);
    setEditingStatusId(null);
    setEditingStatusColorId(null);
  };

  // Loading state
  if (scoped && projectsLoading) {
    return <p role="status">{t('common:states.loading')}</p>;
  }
  if (scoped && projectsError) {
    return (
      <div role="alert">
        <p>
          {t('projects:scoped.loadError', 'Could not load project settings')}
        </p>
        <button onClick={retryProjects}>{t('common:buttons.retry')}</button>
      </div>
    );
  }
  if (scoped && !selectedProject) {
    return <p>{t('projects:scoped.notFound', 'Project not found')}</p>;
  }
  if (!isLoaded || (!scoped && orgsLoading)) {
    return (
      <div className="flex items-center justify-center py-8 gap-2">
        <SpinnerIcon
          className="size-icon-lg animate-spin text-brand"
          weight="bold"
        />
        <span className="text-normal">
          {t('settings.remoteProjects.loading', 'Loading remote projects...')}
        </span>
      </div>
    );
  }

  // Auth check - show sign-in prompt if not signed in
  if (!isSignedIn) {
    return (
      <div className="space-y-4">
        <div>
          <h3 className="text-base font-medium text-high">
            {t(
              'settings.remoteProjects.loginRequired.title',
              'Sign in required'
            )}
          </h3>
          <p className="text-sm text-low mt-1">
            {t(
              'settings.remoteProjects.loginRequired.description',
              'Sign in to manage your remote projects.'
            )}
          </p>
        </div>
        <PrimaryButton
          variant="secondary"
          value={t('settings.remoteProjects.loginRequired.action', 'Sign in')}
          onClick={() => void OAuthDialog.show({})}
        >
          <SignInIcon className="size-icon-xs mr-1" weight="bold" />
        </PrimaryButton>
      </div>
    );
  }

  // Error state
  if (!scoped && orgsError) {
    return (
      <div className="py-8">
        <div className="bg-error/10 border border-error/50 rounded-sm p-4 text-error">
          {orgsError instanceof Error
            ? orgsError.message
            : t(
                'settings.remoteProjects.loadError',
                'Failed to load organizations'
              )}
        </div>
      </div>
    );
  }

  return (
    <>
      {/* Status messages */}
      {error && (
        <div className="bg-error/10 border border-error/50 rounded-sm p-4 text-error mb-4">
          {error}
        </div>
      )}

      {success && (
        <div className="bg-success/10 border border-success/50 rounded-sm p-4 text-success font-medium mb-4">
          {success}
        </div>
      )}

      <SettingsCard
        title={
          scoped
            ? (selectedProject?.name ?? '')
            : t('settings.remoteProjects.title', 'Project settings')
        }
        description={t(
          'settings.remoteProjects.description',
          'Manage project details, boards, and the workspace used on this device.'
        )}
        headerAction={
          scoped && selectedProject ? (
            <ProjectActionsMenu
              protectedProject={isDefaultProject(selectedProject.id)}
              projectName={selectedProject.name}
              className="inline-flex size-9 items-center justify-center rounded-sm border border-border bg-primary text-low hover:bg-secondary hover:text-high focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand disabled:opacity-50 [@media(pointer:coarse)]:size-11"
              disabled={isSaving || pendingProjectId !== null}
              onDelete={() => void handleDeleteProject(selectedProject)}
            />
          ) : undefined
        }
      >
        {/* Two-column picker */}
        {!scoped && (
          <TwoColumnPicker>
            {/* Organizations column */}
            <TwoColumnPickerColumn
              label={t(
                'settings.remoteProjects.columns.organizations',
                'Organizations'
              )}
              isFirst
            >
              {organizations.map((org) => (
                <TwoColumnPickerItem
                  key={org.id}
                  selected={selectedOrgId === org.id}
                  onClick={() => handleOrgSelect(org.id)}
                  trailing={
                    org.is_personal && (
                      <TwoColumnPickerBadge>
                        {t('common:personal', 'Personal')}
                      </TwoColumnPickerBadge>
                    )
                  }
                >
                  {org.name}
                </TwoColumnPickerItem>
              ))}
            </TwoColumnPickerColumn>

            {/* Projects column */}
            <TwoColumnPickerColumn
              label={t('settings.remoteProjects.columns.projects', 'Projects')}
              headerAction={
                selectedOrgId && (
                  <button
                    className="p-half rounded-sm hover:bg-secondary text-low hover:text-normal"
                    onClick={handleCreateProject}
                    disabled={isSaving}
                    title={t(
                      'settings.remoteProjects.actions.addProject',
                      'Add Project'
                    )}
                  >
                    <PlusIcon className="size-icon-2xs" weight="bold" />
                  </button>
                )
              }
            >
              {projectsLoading ? (
                <div className="flex items-center justify-center py-double gap-base">
                  <SpinnerIcon className="size-icon-sm animate-spin" />
                </div>
              ) : selectedOrgId && projects.length > 0 ? (
                projects.map((project) => (
                  <TwoColumnPickerItem
                    key={project.id}
                    selected={selectedProjectId === project.id}
                    onClick={() => handleProjectSelect(project.id)}
                    leading={
                      <span
                        className="w-3 h-3 rounded-full shrink-0"
                        style={{ backgroundColor: `hsl(${project.color})` }}
                      />
                    }
                    trailing={
                      <ProjectActionsMenu
                        protectedProject={isDefaultProject(project.id)}
                        projectName={project.name}
                        className="p-half rounded-sm hover:bg-panel text-low hover:text-normal opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:ring-2 focus-visible:ring-brand [@media(pointer:coarse)]:opacity-100 [@media(pointer:coarse)]:size-11"
                        disabled={isSaving || pendingProjectId !== null}
                        onDelete={() => void handleDeleteProject(project)}
                      />
                    }
                  >
                    {project.name}
                  </TwoColumnPickerItem>
                ))
              ) : selectedOrgId ? (
                <TwoColumnPickerEmpty>
                  {t(
                    'settings.remoteProjects.noProjects',
                    'No projects yet. Create one to get started.'
                  )}
                </TwoColumnPickerEmpty>
              ) : (
                <TwoColumnPickerEmpty>
                  {t(
                    'settings.remoteProjects.selectOrg',
                    'Select an organization'
                  )}
                </TwoColumnPickerEmpty>
              )}
            </TwoColumnPickerColumn>
          </TwoColumnPicker>
        )}

        {/* Edit form (when project selected) */}
        {isDefaultProject(selectedProjectId) && (
          <p className="p-base text-sm text-low">
            {t('common:defaultProject.description')}
          </p>
        )}
        {selectedProjectId &&
          !isDefaultProject(selectedProjectId) &&
          formState && (
            <div className="bg-secondary/50 border border-border rounded-sm p-4 space-y-4">
              <SettingsField
                label={t(
                  'settings.remoteProjects.form.name.label',
                  'Project Name'
                )}
              >
                <SettingsInput
                  value={formState.name}
                  onChange={(name) =>
                    setFormState((s) => (s ? { ...s, name } : null))
                  }
                  placeholder={t(
                    'settings.remoteProjects.form.name.placeholder',
                    'Enter project name'
                  )}
                  disabled={isSaving}
                />
              </SettingsField>

              <SettingsField
                label={t(
                  'settings.remoteProjects.form.color.label',
                  'Project Color'
                )}
              >
                <InlineColorPicker
                  value={formState.color}
                  onChange={(color) =>
                    setFormState((s) => (s ? { ...s, color } : null))
                  }
                  colors={PRESET_COLORS}
                  disabled={isSaving}
                />
              </SettingsField>
            </div>
          )}

        {selectedProjectId && !isDefaultProject(selectedProjectId) && (
          <fieldset
            disabled={!machineState.canMutate || isLoadingDefaults}
            className={cn(
              'bg-secondary/50 border border-border rounded-sm p-4 space-y-base',
              isSaving && 'opacity-60 pointer-events-none'
            )}
          >
            <div>
              {!machineState.canMutate && (
                <p role="status" className="text-sm text-low">
                  {t(
                    'settings.page.states.unavailable',
                    'The selected host is unavailable. Host-owned settings cannot be edited.'
                  )}
                </p>
              )}
              <div className="flex items-center justify-between gap-base">
                <p className="text-sm font-medium text-normal">
                  {t(
                    'settings:settings.remoteProjects.form.defaultRepos.label'
                  )}
                </p>
                <button
                  type="button"
                  onClick={() => void handleChooseProjectWorkspace()}
                  className="shrink-0 cursor-pointer rounded-sm px-base py-half text-sm text-normal transition-colors hover:bg-secondary focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-brand disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {t(
                    'settings:settings.remoteProjects.form.defaultRepos.addButton'
                  )}
                </button>
              </div>
            </div>

            {!isLoadingDefaults && defaultReposError && (
              <div
                className="flex items-start gap-half rounded-sm border border-error/30 bg-error/10 px-base py-half"
                role="alert"
              >
                <WarningCircleIcon
                  className="mt-[1px] size-icon-sm shrink-0 text-error"
                  weight="fill"
                />
                <p className="text-sm text-normal">{defaultReposError}</p>
              </div>
            )}

            {isLoadingDefaults ? (
              <div className="flex items-center gap-half py-half">
                <SpinnerIcon className="size-icon-xs animate-spin" />
              </div>
            ) : !workspaceDefault ? (
              <p className="text-sm text-low">
                {t('settings:settings.remoteProjects.form.defaultRepos.empty')}
              </p>
            ) : workspaceDefault.kind === 'direct_folder' ? (
              <div className="flex items-center gap-base rounded-sm bg-secondary px-base py-half">
                <p
                  className="min-w-0 flex-1 truncate font-mono text-sm text-high"
                  title={workspaceDirectoryLabel}
                >
                  {workspaceDirectoryLabel}
                </p>
                <button
                  type="button"
                  onClick={handleRemoveProjectWorkspace}
                  className="flex size-icon-sm items-center justify-center text-low hover:text-normal"
                  aria-label={t(
                    'settings:settings.remoteProjects.form.defaultRepos.remove'
                  )}
                  title={t(
                    'settings:settings.remoteProjects.form.defaultRepos.remove'
                  )}
                >
                  <XIcon className="size-icon-xs" weight="bold" />
                </button>
              </div>
            ) : (
              <div className="flex items-center gap-base rounded-sm bg-secondary px-base py-half">
                <div className="min-w-0 flex-1">
                  <p
                    className="truncate font-mono text-sm text-high"
                    title={workspaceDirectoryLabel}
                  >
                    {workspaceDirectoryLabel ||
                      workspaceDefaultRepo?.display_name ||
                      workspaceDefaultRepo?.name ||
                      workspaceDefault.repo.repo_id}
                  </p>
                </div>
                <DropdownMenu
                  onOpenChange={(open) => {
                    if (open) {
                      void fetchBranchesForRepo(workspaceDefault.repo.repo_id);
                    }
                  }}
                >
                  <DropdownMenuTrigger asChild>
                    <button
                      type="button"
                      className="flex cursor-pointer items-center gap-1 rounded-sm border border-border bg-panel px-1.5 py-0.5 text-xs text-low transition-colors hover:bg-secondary"
                    >
                      {loadingBranches.has(workspaceDefault.repo.repo_id) ? (
                        <SpinnerIcon className="size-icon-2xs animate-spin" />
                      ) : (
                        <>
                          {workspaceDefault.repo.target_branch}
                          <CaretDownIcon
                            className="size-icon-2xs opacity-50"
                            weight="bold"
                          />
                        </>
                      )}
                    </button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent
                    align="end"
                    className="max-h-60 overflow-y-auto"
                  >
                    {loadingBranches.has(workspaceDefault.repo.repo_id) ? (
                      <div className="flex items-center justify-center px-3 py-2">
                        <SpinnerIcon className="size-icon-xs animate-spin" />
                      </div>
                    ) : (branchCache.get(workspaceDefault.repo.repo_id) ?? [])
                        .length === 0 ? (
                      <div className="px-3 py-2 text-xs text-low">
                        {t(
                          'settings:settings.remoteProjects.form.defaultRepos.noBranches'
                        )}
                      </div>
                    ) : (
                      (
                        branchCache.get(workspaceDefault.repo.repo_id) ?? []
                      ).map((branch) => (
                        <DropdownMenuItem
                          key={branch.name}
                          onClick={() =>
                            handleChangeBranch(
                              workspaceDefault.repo.repo_id,
                              branch.name
                            )
                          }
                        >
                          <div className="flex w-full items-center justify-between gap-2">
                            <span
                              className={cn(
                                'truncate',
                                branch.name ===
                                  workspaceDefault.repo.target_branch &&
                                  'font-medium text-high'
                              )}
                            >
                              {branch.name}
                            </span>
                            <div className="flex shrink-0 items-center gap-1">
                              {branch.is_current && (
                                <span className="rounded-sm border border-border bg-secondary px-1 text-[10px] text-low">
                                  {t(
                                    'settings:settings.remoteProjects.form.defaultRepos.current',
                                    { defaultValue: 'current' }
                                  )}
                                </span>
                              )}
                              {branch.name ===
                                workspaceDefault.repo.target_branch && (
                                <CheckIcon
                                  className="size-icon-2xs text-brand"
                                  weight="bold"
                                />
                              )}
                            </div>
                          </div>
                        </DropdownMenuItem>
                      ))
                    )}
                  </DropdownMenuContent>
                </DropdownMenu>
                <button
                  type="button"
                  onClick={handleRemoveProjectWorkspace}
                  className="flex size-icon-sm items-center justify-center text-low hover:text-normal"
                  aria-label={t(
                    'settings:settings.remoteProjects.form.defaultRepos.remove'
                  )}
                  title={t(
                    'settings:settings.remoteProjects.form.defaultRepos.remove'
                  )}
                >
                  <XIcon className="size-icon-xs" weight="bold" />
                </button>
              </div>
            )}

            <p className="text-sm text-low">
              {t(
                'settings:settings.remoteProjects.form.defaultRepos.description'
              )}
            </p>

            {gitWorkspaceDefault && workspaceDefaultRepo && (
              <Accordion type="single" collapsible>
                <AccordionItem
                  value="repository-execution-settings"
                  className="overflow-hidden rounded-sm border border-border bg-panel/40"
                >
                  <AccordionTrigger className="group gap-base px-base py-base text-left transition-colors hover:bg-secondary/60 focus:outline-none focus:ring-1 focus:ring-inset focus:ring-brand">
                    <SlidersHorizontalIcon
                      className="size-icon-sm shrink-0 text-low"
                      weight="bold"
                    />
                    <div className="min-w-0 flex-1">
                      <p className="text-sm font-medium text-normal">
                        {t(
                          'projects:scoped.repositoryTitle',
                          'Linked repository configuration'
                        )}
                      </p>
                      <p className="mt-quarter text-xs text-low">
                        {t(
                          'projects:scoped.repositoryDescription',
                          'Repository scripts and execution settings are shared by every project using this repository.'
                        )}
                      </p>
                    </div>
                    <CaretDownIcon
                      className="size-icon-xs shrink-0 text-low transition-transform group-data-[state=open]:rotate-180"
                      weight="bold"
                    />
                  </AccordionTrigger>
                  <AccordionContent
                    forceMount
                    className="data-[state=closed]:hidden"
                  >
                    <div className="space-y-base border-t border-border p-base">
                      <SettingsMachineUserSystemProvider>
                        <ReposSettingsSection
                          key={`${machineScopeKey}:${gitWorkspaceDefault.repo_id}`}
                          initialState={{
                            repoId: gitWorkspaceDefault.repo_id,
                          }}
                          variant="project-workspace"
                          onDirtyChange={setRepoConfigDirty}
                        />
                      </SettingsMachineUserSystemProvider>
                    </div>
                  </AccordionContent>
                </AccordionItem>
              </Accordion>
            )}
          </fieldset>
        )}

        {/* Project status settings (kanban columns) */}
        {selectedProjectId && !isDefaultProject(selectedProjectId) && (
          <div
            className={cn(
              'bg-secondary/50 border border-border rounded-sm p-4 space-y-base',
              isSaving && 'opacity-60 pointer-events-none'
            )}
          >
            <div>
              <div>
                <p className="text-sm font-medium text-normal">
                  {t(
                    'settings.remoteProjects.form.statuses.label',
                    'Project Statuses'
                  )}
                </p>
                <p className="text-sm text-low mt-1">
                  {t(
                    'settings.remoteProjects.form.statuses.description',
                    'Manage kanban columns for this project.'
                  )}
                </p>
              </div>
            </div>

            <div className="flex items-center justify-between text-normal">
              <span className="text-sm font-semibold">
                {t('kanban.visibleColumns', 'Visible Columns')}
              </span>
              <span className="text-xs text-low">
                {t('kanban.dragToRearrange', 'Drag to re-arrange')}
              </span>
            </div>

            <DragDropContext onDragEnd={handleStatusDragEnd}>
              <Droppable
                droppableId="status-list"
                renderClone={(
                  provided: DraggableProvided,
                  _snapshot: DraggableStateSnapshot,
                  rubric: DraggableRubric
                ) => (
                  <StatusRowClone
                    provided={provided}
                    status={localStatuses[rubric.source.index]}
                  />
                )}
              >
                {(provided: DroppableProvided) => (
                  <div
                    ref={provided.innerRef}
                    {...provided.droppableProps}
                    className="flex flex-col gap-[2px]"
                  >
                    {localStatuses.map((status, index) => (
                      <StatusRow
                        key={status.id}
                        status={status}
                        index={index}
                        taskCount={taskCountByStatus[status.id] ?? 0}
                        visibleCount={visibleStatusCount}
                        editingId={editingStatusId}
                        editingColorId={editingStatusColorId}
                        onToggleHidden={handleStatusToggleHidden}
                        onNameChange={handleStatusNameChange}
                        onColorChange={handleStatusColorChange}
                        onDelete={handleStatusDelete}
                        onStartEditing={setEditingStatusId}
                        onStartEditingColor={setEditingStatusColorId}
                        onStopEditing={() => setEditingStatusId(null)}
                      />
                    ))}
                    {provided.placeholder}

                    <button
                      type="button"
                      onClick={handleStatusAdd}
                      className="flex items-center gap-half px-base py-half text-high hover:bg-secondary rounded-sm transition-colors"
                    >
                      <div className="flex items-center justify-center size-icon-sm">
                        <PlusIcon className="size-icon-xs" weight="bold" />
                      </div>
                      <span className="text-xs font-light">
                        {t('kanban.addColumn', 'Add column')}
                      </span>
                    </button>
                  </div>
                )}
              </Droppable>
            </DragDropContext>
          </div>
        )}
      </SettingsCard>

      <SettingsSaveBar
        show={isDirty}
        saving={isSaving}
        onSave={handleSave}
        onDiscard={handleDiscard}
      />
    </>
  );
}

// Alias for backwards compatibility
export { RemoteProjectsSettingsSection as RemoteProjectsSettingsSectionContent };
