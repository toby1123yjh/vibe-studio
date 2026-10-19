import { useSyncExternalStore } from 'react';
import type { ProjectStatus, Task } from 'shared/remote-types';
import type { KanbanTaskDescriptionEditorProps } from '@vibe/ui/components/KanbanTaskPanel';

export const statuses: ProjectStatus[] = ['Discuss', 'Todo'].map(
  (name, index) => ({
    id: name.toLowerCase(),
    project_id: 'project-1',
    name,
    color: '215 12% 55%',
    sort_order: index,
    hidden: false,
    created_at: '2026-10-19T00:00:00Z',
  })
);

const subscribers = new Set<() => void>();
let state = { tasks: [] as Task[], taskId: null as string | null };

function update(next: typeof state) {
  state = next;
  subscribers.forEach((subscriber) => subscriber());
}

export function useFixtureState() {
  return useSyncExternalStore(
    (listener) => {
      subscribers.add(listener);
      return () => subscribers.delete(listener);
    },
    () => state
  );
}

export function record(event: string, value?: unknown) {
  const data = document.documentElement.dataset;
  data.events = JSON.stringify([
    ...JSON.parse(data.events ?? '[]'),
    { event, value },
  ]);
}

const empty = () => [];
const project = {
  projectId: 'project-1',
  statuses,
  tags: [],
  taskAssignees: [],
  taskTags: [],
  isLoading: false,
  insertTask(input: object) {
    record('insert', input);
    const persisted = fetch('/__fixture/task', {
      method: 'POST',
      body: JSON.stringify(input),
    }).then(async (response) => {
      if (!response.ok) throw new Error('Task persistence failed');
      const task: Task = await response.json();
      update({ ...state, tasks: [...state.tasks, task] });
      return task;
    });
    return { persisted };
  },
  updateTask: () => record('update'),
  insertTaskAssignee: (value: unknown) => record('assignee', value),
  insertTaskTag: (value: unknown) => record('tag', value),
  removeTaskTag: () => record('remove-tag'),
  insertTag: () => ({ data: { id: 'tag-1' } }),
  getTagsForTask: empty,
  getPullRequestsForTask: empty,
};

export function useProjectContext() {
  return { ...project, tasks: useFixtureState().tasks };
}

const org = {
  organizationId: 'org-1',
  isLoading: false,
  membersWithProfilesById: new Map(),
};
export const useOrgContext = () => org;
const actions = {};
export const useActions = () => actions;

const navigation = {
  goToProjectTask(projectId: string, taskId: string) {
    record('task-details', { projectId, taskId });
    history.pushState(null, '', `/projects/${projectId}/tasks/${taskId}`);
    update({ ...state, taskId });
  },
  goToProject() {
    record('project');
    update({ ...state, taskId: null });
  },
};
export const useAppNavigation = () => navigation;
export function useCurrentKanbanRouteState() {
  return { hostId: null, taskId: useFixtureState().taskId };
}

const attachments = {
  getAttachmentIds: empty,
  clearAttachments: () => undefined,
  uploadFiles: () => undefined,
  isUploading: false,
  hasPendingAttachments: false,
  uploadError: null,
  clearUploadError: () => undefined,
  localAttachments: [],
};
export const useAzureAttachments = () => attachments;
export const commitTaskAttachments = async () => undefined;
export const deleteAttachment = async () => undefined;

export default function Editor({
  value,
  onChange,
}: KanbanTaskDescriptionEditorProps) {
  return (
    <textarea
      aria-label="Task description"
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  );
}

export const ProjectWorkspaceDefaultContext = () => null;
export const SearchableTagDropdownContainer = () => null;
export const TaskCommentsSectionContainer = () => null;
export const TaskSubTasksSectionContainer = () => null;
export const TaskRelationshipsSectionContainer = () => null;
export const TaskArenaSectionContainer = () => null;
export const TaskExecutionAttemptsSectionContainer = () => null;
export const CommandBarDialog = { show: async () => undefined };
export const ProjectSelectionDialog = CommandBarDialog;
export const LinkPrToTaskDialog = CommandBarDialog;
export const AssigneeSelectionDialog = CommandBarDialog;
