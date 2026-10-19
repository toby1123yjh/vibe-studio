import type { TaskPriority } from 'shared/remote-types';
import type {
  TaskFormData,
  TaskPanelMode,
} from '@vibe/ui/components/KanbanTaskPanel';

interface EditTextState {
  title: string;
  hasLocalTitleEdit: boolean;
  description: string | null;
  hasLocalDescriptionEdit: boolean;
}

export interface KanbanTaskPanelFormState {
  createFormData: TaskFormData | null;
  editTextState: EditTextState;
  isDraftAutosavePaused: boolean;
  hasRestoredFromScratch: boolean;
}

interface SelectedTaskSnapshot {
  title: string;
  description: string | null;
  status_id: string;
  priority: TaskPriority | null;
}

type KanbanTaskPanelFormAction =
  | {
      type: 'resetForTaskChange';
      mode: TaskPanelMode;
      createFormData: TaskFormData | null;
      hasRestoredFromScratch: boolean;
    }
  | { type: 'setCreateFormData'; createFormData: TaskFormData | null }
  | {
      type: 'patchCreateFormData';
      patch: Partial<TaskFormData>;
      fallback: TaskFormData;
    }
  | { type: 'setCreateAssigneeIds'; assigneeIds: string[] }
  | { type: 'setDraftAutosavePaused'; isPaused: boolean }
  | {
      type: 'setHasRestoredFromScratch';
      hasRestoredFromScratch: boolean;
    }
  | { type: 'setEditTitle'; title: string }
  | { type: 'setEditDescription'; description: string | null };

const EMPTY_EDIT_TEXT_STATE: EditTextState = {
  title: '',
  hasLocalTitleEdit: false,
  description: null,
  hasLocalDescriptionEdit: false,
};

export function createBlankCreateFormData(
  defaultStatusId: string
): TaskFormData {
  return {
    title: '',
    description: null,
    statusId: defaultStatusId,
    priority: null,
    assigneeIds: [],
    tagIds: [],
  };
}

export function createInitialKanbanTaskPanelFormState(): KanbanTaskPanelFormState {
  return {
    createFormData: null,
    editTextState: EMPTY_EDIT_TEXT_STATE,
    isDraftAutosavePaused: false,
    hasRestoredFromScratch: false,
  };
}

export function kanbanTaskPanelFormReducer(
  state: KanbanTaskPanelFormState,
  action: KanbanTaskPanelFormAction
): KanbanTaskPanelFormState {
  switch (action.type) {
    case 'resetForTaskChange':
      return {
        createFormData: action.mode === 'create' ? action.createFormData : null,
        editTextState: EMPTY_EDIT_TEXT_STATE,
        isDraftAutosavePaused: false,
        hasRestoredFromScratch:
          action.mode === 'create' ? action.hasRestoredFromScratch : false,
      };
    case 'setCreateFormData':
      return {
        ...state,
        createFormData: action.createFormData,
      };
    case 'patchCreateFormData':
      return {
        ...state,
        createFormData: {
          ...(state.createFormData ?? action.fallback),
          ...action.patch,
        },
      };
    case 'setCreateAssigneeIds':
      return {
        ...state,
        createFormData: state.createFormData
          ? {
              ...state.createFormData,
              assigneeIds: action.assigneeIds,
            }
          : state.createFormData,
      };
    case 'setDraftAutosavePaused':
      return {
        ...state,
        isDraftAutosavePaused: action.isPaused,
      };
    case 'setHasRestoredFromScratch':
      return {
        ...state,
        hasRestoredFromScratch: action.hasRestoredFromScratch,
      };
    case 'setEditTitle':
      return {
        ...state,
        editTextState: {
          ...state.editTextState,
          title: action.title,
          hasLocalTitleEdit: true,
        },
      };
    case 'setEditDescription':
      return {
        ...state,
        editTextState: {
          ...state.editTextState,
          description: action.description,
          hasLocalDescriptionEdit: true,
        },
      };
    default:
      return state;
  }
}

function areStringSetsEqual(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false;
  const aSet = new Set(a);
  for (const item of b) {
    if (!aSet.has(item)) return false;
  }
  return true;
}

interface DisplayDataSelectorInput {
  state: KanbanTaskPanelFormState;
  mode: TaskPanelMode;
  createModeDefaults: TaskFormData;
  selectedTask: SelectedTaskSnapshot | null;
  currentAssigneeIds: string[];
  currentTagIds: string[];
}

export function selectDisplayData({
  state,
  mode,
  createModeDefaults,
  selectedTask,
  currentAssigneeIds,
  currentTagIds,
}: DisplayDataSelectorInput): TaskFormData {
  if (mode === 'create') {
    return state.createFormData ?? createModeDefaults;
  }

  return {
    title: state.editTextState.hasLocalTitleEdit
      ? state.editTextState.title
      : (selectedTask?.title ?? ''),
    description: state.editTextState.hasLocalDescriptionEdit
      ? state.editTextState.description
      : (selectedTask?.description ?? null),
    statusId: selectedTask?.status_id ?? '',
    priority: selectedTask?.priority ?? null,
    assigneeIds: currentAssigneeIds,
    tagIds: currentTagIds,
  };
}

interface CreateDraftDirtySelectorInput {
  state: KanbanTaskPanelFormState;
  mode: TaskPanelMode;
  createModeDefaults: TaskFormData;
}

export function selectIsCreateDraftDirty({
  state,
  mode,
  createModeDefaults,
}: CreateDraftDirtySelectorInput): boolean {
  if (mode !== 'create' || !state.createFormData) return false;

  return (
    state.createFormData.title !== createModeDefaults.title ||
    (state.createFormData.description ?? null) !==
      createModeDefaults.description ||
    state.createFormData.statusId !== createModeDefaults.statusId ||
    state.createFormData.priority !== createModeDefaults.priority ||
    !areStringSetsEqual(
      state.createFormData.assigneeIds,
      createModeDefaults.assigneeIds
    ) ||
    !areStringSetsEqual(state.createFormData.tagIds, createModeDefaults.tagIds)
  );
}
