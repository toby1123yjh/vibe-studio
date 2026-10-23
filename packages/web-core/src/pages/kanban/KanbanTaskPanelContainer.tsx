import {
  useState,
  useCallback,
  useEffect,
  useReducer,
  useRef,
  useMemo,
} from 'react';
import { useDropzone } from 'react-dropzone';
import { useTranslation } from 'react-i18next';
import type { OrganizationMemberWithProfile } from 'shared/types';
import type { TaskPriority } from 'shared/remote-types';
import { useDebouncedCallback } from '@/shared/hooks/useDebouncedCallback';
import { useProjectContext } from '@/shared/hooks/useProjectContext';
import { useOrgContext } from '@/shared/hooks/useOrgContext';
import WYSIWYGEditor from '@/shared/components/WYSIWYGEditor';
import { SearchableTagDropdownContainer } from '@/shared/components/SearchableTagDropdownContainer';
import { TaskCommentsSectionContainer } from './TaskCommentsSectionContainer';
import { TaskSubTasksSectionContainer } from './TaskSubTasksSectionContainer';
import { TaskRelationshipsSectionContainer } from './TaskRelationshipsSectionContainer';
import { TaskArenaSectionContainer } from './TaskArenaSectionContainer';
import { TaskExecutionAttemptsSectionContainer } from './TaskExecutionAttemptsSectionContainer';
import {
  KanbanTaskPanel,
  type TaskFormData,
} from '@vibe/ui/components/KanbanTaskPanel';
import { useActions } from '@/shared/hooks/useActions';
import { CommandBarDialog } from '@/shared/dialogs/command-bar/CommandBarDialog';
import {
  createBlankCreateFormData,
  createInitialKanbanTaskPanelFormState,
  kanbanTaskPanelFormReducer,
  selectDisplayData,
  selectIsCreateDraftDirty,
} from './kanban-task-panel-state';
import { useAzureAttachments } from '@/shared/hooks/useAzureAttachments';
import {
  commitTaskAttachments,
  deleteAttachment,
} from '@/shared/lib/remoteApi';
import {
  extractAttachmentIds,
  removeAttachmentMarkdownBySource,
  replaceAttachmentSource,
} from '@/shared/lib/attachmentUtils';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useCurrentKanbanRouteState } from '@/shared/hooks/useCurrentKanbanRouteState';
import {
  buildKanbanTaskComposerKey,
  closeKanbanTaskComposer,
  patchKanbanTaskComposer,
  resetKanbanTaskComposer,
  useKanbanTaskComposer,
  useKanbanTaskComposerStore,
} from '@/shared/stores/useKanbanTaskComposerStore';

interface KanbanTaskPanelContainerProps {
  taskResolution: 'resolving' | 'ready' | 'missing' | null;
  onExpectTaskOpen: (taskId: string) => void;
}

/**
 * KanbanIssuePanelContainer manages the issue detail/create panel.
 * Uses ProjectContext and OrgContext for data and mutations.
 * Must be rendered within both OrgProvider and ProjectProvider.
 */
export function KanbanTaskPanelContainer({
  taskResolution,
  onExpectTaskOpen,
}: KanbanTaskPanelContainerProps) {
  const { t } = useTranslation('common');
  const appNavigation = useAppNavigation();
  const routeState = useCurrentKanbanRouteState();

  // Get data from contexts
  const {
    projectId,
    tasks,
    statuses,
    tags,
    taskAssignees,
    taskTags,
    insertTask,
    updateTask,
    insertTaskAssignee,
    insertTaskTag,
    removeTaskTag,
    insertTag,
    getTagsForTask,
    getPullRequestsForTask,
    isLoading: projectLoading,
  } = useProjectContext();
  const selectedKanbanTaskId = routeState.taskId;
  const taskComposerKey = useMemo(
    () => buildKanbanTaskComposerKey(routeState.hostId, projectId),
    [routeState.hostId, projectId]
  );
  const taskComposer = useKanbanTaskComposer(taskComposerKey);
  const kanbanCreateMode = taskComposer !== null;
  const createComposerInitial = taskComposer?.initial ?? null;
  const kanbanCreateDefaultStatusId = createComposerInitial?.statusId ?? null;
  const kanbanCreateDefaultPriority = createComposerInitial?.priority ?? null;
  const kanbanCreateDefaultAssigneeIds =
    createComposerInitial?.assigneeIds ?? null;
  const kanbanCreateDefaultParentTaskId =
    createComposerInitial?.parentTaskId ?? null;
  const openTask = useCallback(
    (taskId: string) => {
      if (kanbanCreateMode && taskComposerKey) {
        closeKanbanTaskComposer(taskComposerKey);
      }
      appNavigation.goToProjectTask(projectId, taskId);
    },
    [kanbanCreateMode, taskComposerKey, appNavigation, projectId]
  );
  const closeKanbanTaskPanel = useCallback(() => {
    if (kanbanCreateMode && taskComposerKey) {
      closeKanbanTaskComposer(taskComposerKey);
    }
    appNavigation.goToProject(projectId);
  }, [kanbanCreateMode, taskComposerKey, appNavigation, projectId]);
  const updateTaskComposerDraft = useCallback(
    (patch: {
      statusId?: string;
      priority?: TaskPriority | null;
      assigneeIds?: string[];
      parentTaskId?: string;
      title?: string;
      description?: string | null;
      tagIds?: string[];
    }) => {
      if (!kanbanCreateMode || !taskComposerKey) {
        return;
      }

      patchKanbanTaskComposer(taskComposerKey, patch);
    },
    [kanbanCreateMode, taskComposerKey]
  );
  const resetTaskComposerDraft = useCallback(() => {
    if (!taskComposerKey) {
      return;
    }

    resetKanbanTaskComposer(taskComposerKey);
  }, [taskComposerKey]);

  const { isLoading: orgLoading, membersWithProfilesById } = useOrgContext();

  // Get action methods from actions context
  const { openStatusSelection, openPrioritySelection, openAssigneeSelection } =
    useActions();

  // Find selected issue if in edit mode
  const selectedTask = useMemo(() => {
    if (kanbanCreateMode || !selectedKanbanTaskId) return null;
    return tasks.find((i) => i.id === selectedKanbanTaskId) ?? null;
  }, [tasks, selectedKanbanTaskId, kanbanCreateMode]);

  const creatorUserId = selectedTask?.creator_user_id ?? null;
  const taskCreator = useMemo(() => {
    if (!creatorUserId) return null;
    return membersWithProfilesById.get(creatorUserId) ?? null;
  }, [membersWithProfilesById, creatorUserId]);

  // Find parent issue if current issue has one
  const parentTask = useMemo(() => {
    if (!selectedTask?.parent_task_id) return null;
    const parent = tasks.find((i) => i.id === selectedTask.parent_task_id);
    if (!parent) return null;
    return { id: parent.id, simpleId: parent.simple_id };
  }, [tasks, selectedTask]);

  // Handler for clicking on parent issue - navigate to that issue
  const handleParentTaskClick = useCallback(() => {
    if (parentTask) {
      openTask(parentTask.id);
    }
  }, [parentTask, openTask]);

  const handleRemoveParentTask = useCallback(() => {
    if (!selectedKanbanTaskId || !selectedTask?.parent_task_id) return;
    updateTask(selectedKanbanTaskId, {
      parent_task_id: null,
      parent_task_sort_order: null,
    });
  }, [selectedKanbanTaskId, selectedTask?.parent_task_id, updateTask]);

  // Get all current assignees from issue_assignees
  const currentAssigneeIds = useMemo(() => {
    if (!selectedKanbanTaskId) return [];
    return taskAssignees
      .filter((a) => a.task_id === selectedKanbanTaskId)
      .map((a) => a.user_id);
  }, [taskAssignees, selectedKanbanTaskId]);

  // Get current tag IDs from issue_tags junction table
  const currentTagIds = useMemo(() => {
    if (!selectedKanbanTaskId) return [];
    const tagLinks = getTagsForTask(selectedKanbanTaskId);
    return tagLinks.map((it) => it.tag_id);
  }, [getTagsForTask, selectedKanbanTaskId]);

  // Get linked PRs for the issue
  const linkedPrs = useMemo(() => {
    if (!selectedKanbanTaskId) return [];
    return getPullRequestsForTask(selectedKanbanTaskId).map((pr) => ({
      id: pr.id,
      number: pr.number,
      url: pr.url,
      status: pr.status,
    }));
  }, [getPullRequestsForTask, selectedKanbanTaskId]);

  // Determine mode from composer state (create) or issue route (edit).
  const mode = kanbanCreateMode ? 'create' : 'edit';

  // Sort statuses by sort_order
  const sortedStatuses = useMemo(
    () => [...statuses].sort((a, b) => a.sort_order - b.sort_order),
    [statuses]
  );

  // Default status: use kanbanCreateDefaultStatusId if set, otherwise first by sort order
  const defaultStatusId =
    kanbanCreateDefaultStatusId ?? sortedStatuses[0]?.id ?? '';

  // Default create form values for the current create-default state + project context
  const createModeDefaults = useMemo<TaskFormData>(
    () => ({
      title: '',
      description: null,
      statusId: defaultStatusId,
      priority: kanbanCreateDefaultPriority ?? null,
      assigneeIds: [...(kanbanCreateDefaultAssigneeIds ?? [])],
      tagIds: [],
    }),
    [
      defaultStatusId,
      kanbanCreateDefaultPriority,
      kanbanCreateDefaultAssigneeIds,
    ]
  );

  // Track previous issue ID to detect actual issue switches (not just data updates)
  const prevTaskIdRef = useRef<string | null>(null);
  const prevHasPendingAttachmentsRef = useRef(false);
  const hasPendingAttachmentsRef = useRef(false);
  const titleInputRef = useRef<HTMLTextAreaElement>(null);

  const [formState, dispatchFormState] = useReducer(
    kanbanTaskPanelFormReducer,
    undefined,
    createInitialKanbanTaskPanelFormState
  );
  const createFormData = formState.createFormData;

  useEffect(() => {
    if (mode !== 'create') return;

    const titleInput = titleInputRef.current;
    if (!titleInput || document.activeElement === titleInput) return;

    const frameId = requestAnimationFrame(() => {
      const node = titleInputRef.current;
      if (!node || document.activeElement === node) return;

      node.focus();
      const caretIndex = node.value.length;
      node.setSelectionRange(caretIndex, caretIndex);
    });

    return () => cancelAnimationFrame(frameId);
  }, [mode, selectedKanbanTaskId, createFormData?.title]);

  // Display ID: use real simple_id in edit mode, placeholder for create mode
  const displayId = useMemo(() => {
    if (mode === 'edit' && selectedTask) {
      return selectedTask.simple_id;
    }
    return t('kanban.newIssue');
  }, [mode, selectedTask, t]);

  // Compute display values based on mode
  // - Create mode: createFormData is the single source of truth.
  // - Edit mode: text fields come from explicit local edit state, dropdown fields from server.
  const displayData = useMemo((): TaskFormData => {
    return selectDisplayData({
      state: formState,
      mode,
      createModeDefaults,
      selectedTask,
      currentAssigneeIds,
      currentTagIds,
    });
  }, [
    formState,
    mode,
    createModeDefaults,
    selectedTask,
    currentAssigneeIds,
    currentTagIds,
  ]);
  const latestDescriptionRef = useRef<string | null>(
    displayData.description ?? null
  );
  latestDescriptionRef.current = displayData.description ?? null;

  const isCreateDraftDirty = useMemo(() => {
    return selectIsCreateDraftDirty({
      state: formState,
      mode,
      createModeDefaults,
    });
  }, [formState, mode, createModeDefaults]);

  // Resolve assignee IDs to full profiles for avatar display
  const displayAssigneeUsers = useMemo(() => {
    return displayData.assigneeIds
      .map((id) => membersWithProfilesById.get(id))
      .filter((m): m is OrganizationMemberWithProfile => m != null);
  }, [displayData.assigneeIds, membersWithProfilesById]);

  const [isSubmitting, setIsSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  // Save status for description (shown in WYSIWYG toolbar)
  const [descriptionSaveStatus, setDescriptionSaveStatus] = useState<
    'idle' | 'saved'
  >('idle');

  // Debounced save for title changes
  const { debounced: debouncedSaveTitle, cancel: cancelDebouncedTitle } =
    useDebouncedCallback((title: string) => {
      if (selectedKanbanTaskId && !kanbanCreateMode) {
        updateTask(selectedKanbanTaskId, { title });
      }
    }, 500);

  // Debounced save for description changes
  const {
    debounced: debouncedSaveDescription,
    cancel: cancelDebouncedDescription,
  } = useDebouncedCallback((description: string | null) => {
    if (selectedKanbanTaskId && !kanbanCreateMode) {
      updateTask(selectedKanbanTaskId, { description });
      setDescriptionSaveStatus('saved');
      setTimeout(() => setDescriptionSaveStatus('idle'), 1500);
    }
  }, 500);

  // Reset save status only when switching to a different issue or mode
  useEffect(() => {
    setDescriptionSaveStatus('idle');
  }, [selectedKanbanTaskId, kanbanCreateMode]);

  const createFormFallback = useMemo(
    () => createBlankCreateFormData(defaultStatusId),
    [defaultStatusId]
  );

  // --- Image attachment upload integration ---

  // Callback to insert markdown into the description field
  const handleDescriptionInsert = useCallback(
    (markdown: string, options?: { persist?: boolean }) => {
      const currentDesc = latestDescriptionRef.current ?? '';
      const separator = currentDesc.length > 0 ? '\n' : '';
      const newDesc = currentDesc + separator + markdown;
      latestDescriptionRef.current = newDesc;

      if (kanbanCreateMode || !selectedKanbanTaskId) {
        // Create mode: update form data
        dispatchFormState({
          type: 'patchCreateFormData',
          patch: { description: newDesc },
          fallback: createFormFallback,
        });
      } else {
        // Edit mode: update local state + debounced save
        dispatchFormState({
          type: 'setEditDescription',
          description: newDesc,
        });
        if (options?.persist !== false && !hasPendingAttachmentsRef.current) {
          debouncedSaveDescription(newDesc);
        }
      }
    },
    [
      kanbanCreateMode,
      selectedKanbanTaskId,
      createFormFallback,
      debouncedSaveDescription,
    ]
  );

  const handleDescriptionSourceReplace = useCallback(
    (previousSrc: string, nextSrc: string, options?: { persist?: boolean }) => {
      const currentDesc = latestDescriptionRef.current ?? '';
      const { content: nextDesc, replaced } = replaceAttachmentSource(
        currentDesc,
        previousSrc,
        nextSrc
      );

      if (!replaced) {
        return false;
      }
      latestDescriptionRef.current = nextDesc;

      if (kanbanCreateMode || !selectedKanbanTaskId) {
        dispatchFormState({
          type: 'patchCreateFormData',
          patch: { description: nextDesc },
          fallback: createFormFallback,
        });
      } else {
        dispatchFormState({
          type: 'setEditDescription',
          description: nextDesc,
        });
        if (options?.persist !== false && !hasPendingAttachmentsRef.current) {
          debouncedSaveDescription(nextDesc);
        }
      }

      return true;
    },
    [
      kanbanCreateMode,
      selectedKanbanTaskId,
      createFormFallback,
      debouncedSaveDescription,
    ]
  );

  const handleDescriptionSourceRemove = useCallback(
    (src: string, options?: { persist?: boolean }) => {
      const currentDesc = latestDescriptionRef.current ?? '';
      const { content: nextDesc, removed } = removeAttachmentMarkdownBySource(
        currentDesc,
        src
      );

      if (!removed) {
        return false;
      }
      latestDescriptionRef.current = nextDesc || null;

      if (kanbanCreateMode || !selectedKanbanTaskId) {
        dispatchFormState({
          type: 'patchCreateFormData',
          patch: { description: nextDesc || null },
          fallback: createFormFallback,
        });
      } else {
        dispatchFormState({
          type: 'setEditDescription',
          description: nextDesc || null,
        });
        if (options?.persist !== false && !hasPendingAttachmentsRef.current) {
          debouncedSaveDescription(nextDesc || null);
        }
      }

      return true;
    },
    [
      kanbanCreateMode,
      selectedKanbanTaskId,
      createFormFallback,
      debouncedSaveDescription,
    ]
  );

  // Azure attachment upload hook
  const {
    uploadFiles,
    getAttachmentIds,
    clearAttachments,
    isUploading,
    hasPendingAttachments,
    uploadError,
    clearUploadError,
    localAttachments,
  } = useAzureAttachments({
    projectId,
    taskId: kanbanCreateMode ? undefined : (selectedKanbanTaskId ?? undefined),
    onMarkdownInsert: handleDescriptionInsert,
    onAttachmentSourceReplace: handleDescriptionSourceReplace,
    onAttachmentSourceRemove: handleDescriptionSourceRemove,
    onError: (msg) => console.error('[attachment]', msg),
  });
  hasPendingAttachmentsRef.current = hasPendingAttachments;

  // Dropzone for drag-drop image upload on description area
  const {
    getRootProps,
    getInputProps,
    isDragActive,
    open: openFilePicker,
  } = useDropzone({
    onDrop: (acceptedFiles) => {
      if (acceptedFiles.length > 0) uploadFiles(acceptedFiles);
    },
    multiple: true,
    noClick: true,
    noKeyboard: true,
  });

  // Paste handler for images
  const onPasteFiles = useCallback(
    (files: File[]) => {
      if (files.length > 0) uploadFiles(files);
    },
    [uploadFiles]
  );

  // Reset local state when switching issues or modes.
  useEffect(() => {
    const currentTaskId = selectedKanbanTaskId;
    const isNewTask = currentTaskId !== prevTaskIdRef.current;
    const shouldSeedCreateForm = mode === 'create' && createFormData === null;

    if (!isNewTask && !shouldSeedCreateForm) {
      // Same issue - no reset needed
      // (dropdown fields derive from server state, text fields preserve local edits)
      return;
    }

    // Track the new issue ID
    prevTaskIdRef.current = currentTaskId;

    // Cancel any pending debounced saves when switching issues
    cancelDebouncedTitle();
    cancelDebouncedDescription();

    let nextCreateFormData: TaskFormData | null = null;
    let restoredFromScratch = false;

    if (mode === 'create') {
      // Check if the composer store has a saved draft (e.g., restored from
      // localStorage on remote-web). Use it to seed the form instead of defaults.
      const composerDraft =
        useKanbanTaskComposerStore.getState().byKey[taskComposerKey]?.draft;
      const hasSavedDraft =
        composerDraft != null &&
        (composerDraft.title !== '' || composerDraft.description != null);

      if (hasSavedDraft) {
        nextCreateFormData = {
          title: composerDraft.title,
          description: composerDraft.description ?? null,
          statusId: composerDraft.statusId ?? createModeDefaults.statusId,
          priority:
            composerDraft.priority === undefined
              ? createModeDefaults.priority
              : composerDraft.priority,
          assigneeIds:
            composerDraft.assigneeIds ?? createModeDefaults.assigneeIds,
          tagIds: composerDraft.tagIds ?? createModeDefaults.tagIds,
        };
        restoredFromScratch = true;
      } else {
        nextCreateFormData = createModeDefaults;
      }
    }

    dispatchFormState({
      type: 'resetForTaskChange',
      mode,
      createFormData: nextCreateFormData,
      hasRestoredFromScratch: restoredFromScratch,
    });
  }, [
    mode,
    createFormData,
    selectedKanbanTaskId,
    cancelDebouncedTitle,
    cancelDebouncedDescription,
    createModeDefaults,
    taskComposerKey,
  ]);

  useEffect(() => {
    const wasPending = prevHasPendingAttachmentsRef.current;
    prevHasPendingAttachmentsRef.current = hasPendingAttachments;

    if (kanbanCreateMode || !selectedKanbanTaskId) {
      return;
    }

    if (!wasPending || hasPendingAttachments) {
      return;
    }

    const currentDescription = displayData.description ?? null;
    const persistedDescription = selectedTask?.description ?? null;

    if (currentDescription === persistedDescription) {
      return;
    }

    debouncedSaveDescription(currentDescription);
  }, [
    kanbanCreateMode,
    selectedKanbanTaskId,
    hasPendingAttachments,
    displayData.description,
    selectedTask?.description,
    debouncedSaveDescription,
  ]);

  // Form change handler - persists changes immediately in edit mode
  const handlePropertyChange = useCallback(
    async <K extends keyof TaskFormData>(field: K, value: TaskFormData[K]) => {
      setSubmitError(null);

      // Create mode: update in-panel form state and composer draft.
      if (kanbanCreateMode) {
        // For statusId, open the status selection dialog with callback
        if (field === 'statusId') {
          const { ProjectSelectionDialog } = await import(
            '@/shared/dialogs/command-bar/selections/ProjectSelectionDialog'
          );
          const result = await ProjectSelectionDialog.show({
            projectId,
            selection: { type: 'status', taskIds: [], isCreateMode: true },
          });
          if (result && typeof result === 'object' && 'statusId' in result) {
            const statusId = result.statusId as string;
            updateTaskComposerDraft({ statusId });
            dispatchFormState({
              type: 'patchCreateFormData',
              patch: { statusId },
              fallback: createFormFallback,
            });
          }
          return;
        }

        // For priority, open the priority selection dialog with callback
        if (field === 'priority') {
          const { ProjectSelectionDialog } = await import(
            '@/shared/dialogs/command-bar/selections/ProjectSelectionDialog'
          );
          const result = await ProjectSelectionDialog.show({
            projectId,
            selection: { type: 'priority', taskIds: [], isCreateMode: true },
          });
          if (result && typeof result === 'object' && 'priority' in result) {
            const priority = (result as { priority: TaskPriority | null })
              .priority;
            updateTaskComposerDraft({ priority });
            dispatchFormState({
              type: 'patchCreateFormData',
              patch: { priority },
              fallback: createFormFallback,
            });
          }
          return;
        }

        // For assigneeIds, open the assignee selection dialog with callback
        if (field === 'assigneeIds') {
          const { AssigneeSelectionDialog } = await import(
            '@/shared/dialogs/kanban/AssigneeSelectionDialog'
          );
          await AssigneeSelectionDialog.show({
            projectId,
            taskIds: [],
            isCreateMode: true,
            createModeAssigneeIds: createFormData?.assigneeIds ?? [],
            onCreateModeAssigneesChange: (assigneeIds: string[]) => {
              updateTaskComposerDraft({ assigneeIds });
              dispatchFormState({
                type: 'setCreateAssigneeIds',
                assigneeIds,
              });
            },
          });
          return;
        }

        // For other fields, just update the form data
        dispatchFormState({
          type: 'patchCreateFormData',
          patch: { [field]: value } as Partial<TaskFormData>,
          fallback: createFormFallback,
        });
        updateTaskComposerDraft({ [field]: value } as Partial<TaskFormData>);
        return;
      }

      if (!selectedKanbanTaskId) {
        return;
      }

      // Edit mode: handle text fields vs dropdown fields differently
      if (field === 'title') {
        // Text field: update local state, then debounced save
        dispatchFormState({
          type: 'setEditTitle',
          title: value as string,
        });
        debouncedSaveTitle(value as string);
      } else if (field === 'description') {
        // Text field: update local state, then debounced save
        dispatchFormState({
          type: 'setEditDescription',
          description: value as string | null,
        });
        if (!hasPendingAttachments) {
          debouncedSaveDescription(value as string | null);
        }
      } else if (field === 'statusId') {
        // Status changes go through the command bar status selection
        openStatusSelection(projectId, [selectedKanbanTaskId]);
      } else if (field === 'priority') {
        // Priority changes go through the command bar priority selection
        openPrioritySelection(projectId, [selectedKanbanTaskId]);
      } else if (field === 'assigneeIds') {
        // Assignee changes go through the assignee selection dialog
        openAssigneeSelection(projectId, [selectedKanbanTaskId], false);
      } else if (field === 'tagIds') {
        // Handle tag changes via junction table
        const newTagIds = value as string[];
        const currentTaskTags = taskTags.filter(
          (it) => it.task_id === selectedKanbanTaskId
        );
        const currentTagIdSet = new Set(currentTaskTags.map((it) => it.tag_id));
        const newTagIdSet = new Set(newTagIds);

        // Remove tags that are no longer selected
        for (const taskTag of currentTaskTags) {
          if (!newTagIdSet.has(taskTag.tag_id)) {
            removeTaskTag(taskTag.id);
          }
        }

        // Add newly selected tags
        for (const tagId of newTagIds) {
          if (!currentTagIdSet.has(tagId)) {
            insertTaskTag({
              task_id: selectedKanbanTaskId,
              tag_id: tagId,
            });
          }
        }
      }
    },
    [
      kanbanCreateMode,
      selectedKanbanTaskId,
      projectId,
      createFormFallback,
      createFormData,
      hasPendingAttachments,
      debouncedSaveTitle,
      debouncedSaveDescription,
      openStatusSelection,
      openPrioritySelection,
      openAssigneeSelection,
      updateTaskComposerDraft,
      taskTags,
      insertTaskTag,
      removeTaskTag,
    ]
  );

  // Submit handler
  const handleSubmit = useCallback(async () => {
    if (!displayData.title.trim() || hasPendingAttachments) return;

    setSubmitError(null);
    setIsSubmitting(true);
    try {
      if (mode === 'create') {
        // Create new issue at the top of the column
        const statusTasks = tasks.filter(
          (i) => i.status_id === displayData.statusId
        );
        const minSortOrder =
          statusTasks.length > 0
            ? Math.min(...statusTasks.map((i) => i.sort_order))
            : 0;

        const { persisted } = insertTask({
          project_id: projectId,
          status_id: displayData.statusId,
          title: displayData.title,
          description: displayData.description,
          priority: displayData.priority,
          sort_order: minSortOrder - 1,
          start_date: null,
          target_date: null,
          completed_at: null,
          parent_task_id: kanbanCreateDefaultParentTaskId,
          parent_task_sort_order: null,
          extension_metadata: null,
        });

        // Wait for the issue to be confirmed by the backend and get the synced entity
        const syncedTask = await persisted;

        // Commit only attachments still referenced in the description
        const allUploadedIds = getAttachmentIds();
        if (allUploadedIds.length > 0) {
          const referencedIds = extractAttachmentIds(
            displayData.description ?? ''
          );
          const idsToCommit = allUploadedIds.filter((id) =>
            referencedIds.has(id)
          );
          const idsToDelete = allUploadedIds.filter(
            (id) => !referencedIds.has(id)
          );

          if (idsToCommit.length > 0) {
            await commitTaskAttachments(syncedTask.id, {
              attachment_ids: idsToCommit,
            });
          }
          for (const id of idsToDelete) {
            deleteAttachment(id).catch((err) =>
              console.error('Failed to delete abandoned attachment:', err)
            );
          }
          clearAttachments();
        }

        // Create assignee records for all selected assignees
        displayData.assigneeIds.forEach((userId) => {
          insertTaskAssignee({
            task_id: syncedTask.id,
            user_id: userId,
          });
        });

        // Create tag records if tags were selected
        for (const tagId of displayData.tagIds) {
          insertTaskTag({
            task_id: syncedTask.id,
            tag_id: tagId,
          });
        }

        // Creation only opens Task details. The user chooses an execution
        // there; keep the expectation, composer close and navigation together.
        onExpectTaskOpen(syncedTask.id);
        openTask(syncedTask.id);
      } else {
        // Update existing issue - would use update mutation
        // For now, just close the panel
        closeKanbanTaskPanel();
      }
    } catch (error) {
      console.error('Failed to save task:', error);
      setSubmitError(
        error instanceof Error ? error.message : 'Failed to save task'
      );
    } finally {
      setIsSubmitting(false);
    }
  }, [
    mode,
    displayData,
    projectId,
    tasks,
    insertTask,
    insertTaskAssignee,
    insertTaskTag,
    openTask,
    kanbanCreateDefaultParentTaskId,
    closeKanbanTaskPanel,
    getAttachmentIds,
    clearAttachments,
    hasPendingAttachments,
    onExpectTaskOpen,
  ]);

  useEffect(() => {
    setSubmitError(null);
  }, [mode, selectedKanbanTaskId, projectId]);

  const handleCmdEnterSubmit = useCallback(() => {
    if (mode !== 'create') return;
    void handleSubmit();
  }, [mode, handleSubmit]);

  const handleDeleteDraft = useCallback(() => {
    dispatchFormState({
      type: 'setCreateFormData',
      createFormData: createModeDefaults,
    });
    resetTaskComposerDraft();
  }, [createModeDefaults, resetTaskComposerDraft]);

  // Tag create callback - returns the new tag ID so it can be auto-selected
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

  // Copy link callback - copies issue URL to clipboard
  const handleCopyLink = useCallback(() => {
    if (!selectedKanbanTaskId || !projectId) return;
    const url = new URL(
      `/projects/${projectId}/tasks/${selectedKanbanTaskId}`,
      window.location.origin
    );
    if (routeState.hostId) url.searchParams.set('host_id', routeState.hostId);
    navigator.clipboard.writeText(url.toString());
  }, [projectId, selectedKanbanTaskId, routeState.hostId]);

  // More actions callback - opens command bar with issue actions
  const handleMoreActions = useCallback(async () => {
    if (!selectedKanbanTaskId || !projectId) return;
    await CommandBarDialog.show({
      page: 'taskActions',
      projectId,
      taskIds: [selectedKanbanTaskId],
    });
  }, [selectedKanbanTaskId, projectId]);

  // Link PR callback - opens link PR dialog
  const handleLinkPr = useCallback(async () => {
    if (!selectedKanbanTaskId) return;
    const { LinkPrToTaskDialog } = await import(
      '@/shared/dialogs/command-bar/LinkPrToTaskDialog'
    );
    await LinkPrToTaskDialog.show({
      projectId,
      taskId: selectedKanbanTaskId,
    });
  }, [selectedKanbanTaskId, projectId]);

  // Loading state
  const isLoading = projectLoading || orgLoading;
  const isResolvingExpectedTask =
    mode === 'edit' &&
    selectedKanbanTaskId !== null &&
    taskResolution === 'resolving';
  const hasMissingTaskDataInEditMode =
    mode === 'edit' && selectedKanbanTaskId !== null && selectedTask === null;

  if (isLoading || isResolvingExpectedTask || hasMissingTaskDataInEditMode) {
    return (
      <div className="flex items-center justify-center h-full bg-secondary">
        <p className="text-low">{t('states.loading')}</p>
      </div>
    );
  }

  return (
    <KanbanTaskPanel
      mode={mode}
      displayId={displayId}
      formData={displayData}
      assigneeUsers={displayAssigneeUsers}
      onFormChange={handlePropertyChange}
      statuses={sortedStatuses}
      tags={tags}
      taskId={selectedKanbanTaskId}
      creatorUser={taskCreator}
      parentTask={parentTask}
      onParentTaskClick={handleParentTaskClick}
      onRemoveParentTask={handleRemoveParentTask}
      linkedPrs={linkedPrs}
      onLinkPr={mode === 'edit' ? handleLinkPr : undefined}
      onClose={closeKanbanTaskPanel}
      onSubmit={handleSubmit}
      onCmdEnterSubmit={handleCmdEnterSubmit}
      onCreateTag={handleCreateTag}
      renderAddTagControl={({
        tags,
        selectedTagIds,
        onTagToggle,
        onCreateTag,
        disabled,
        trigger,
      }) => (
        <SearchableTagDropdownContainer
          tags={tags}
          selectedTagIds={selectedTagIds}
          onTagToggle={onTagToggle}
          onCreateTag={onCreateTag}
          disabled={disabled}
          contentClassName=""
          trigger={trigger}
        />
      )}
      isSubmitting={isSubmitting}
      submitError={submitError}
      onDismissSubmitError={() => setSubmitError(null)}
      descriptionSaveStatus={
        mode === 'edit' ? descriptionSaveStatus : undefined
      }
      titleInputRef={titleInputRef}
      onDeleteDraft={
        mode === 'create' && isCreateDraftDirty ? handleDeleteDraft : undefined
      }
      onCopyLink={mode === 'edit' ? handleCopyLink : undefined}
      onMoreActions={mode === 'edit' ? handleMoreActions : undefined}
      onPasteFiles={onPasteFiles}
      localAttachments={localAttachments}
      dropzoneProps={{ getRootProps, getInputProps, isDragActive }}
      onBrowseAttachment={openFilePicker}
      isUploading={isUploading}
      attachmentError={uploadError}
      onDismissAttachmentError={clearUploadError}
      renderDescriptionEditor={(props) => (
        <WYSIWYGEditor {...props} localAttachments={localAttachments} />
      )}
      renderWorkspacesSection={(taskId) => (
        <>
          <TaskExecutionAttemptsSectionContainer
            taskId={taskId}
            taskTitle={displayData.title}
            taskDescription={displayData.description}
          />
          <TaskArenaSectionContainer taskId={taskId} />
        </>
      )}
      renderRelationshipsSection={(taskId) => (
        <TaskRelationshipsSectionContainer taskId={taskId} />
      )}
      renderSubTasksSection={(taskId) => (
        <TaskSubTasksSectionContainer taskId={taskId} />
      )}
      renderCommentsSection={(taskId) => (
        <TaskCommentsSectionContainer taskId={taskId} />
      )}
    />
  );
}
