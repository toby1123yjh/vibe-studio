import type { ExecutionPageOptions } from "@/shared/lib/executionDataApi";
import type { Task } from "shared/remote-types";

const noop = () => undefined;
const task: Task = {
  id: "issue-1",
  project_id: "project-1",
  task_number: 1,
  simple_id: "VK-1",
  status_id: "todo",
  title: "Observe the running session",
  description: null,
  priority: null,
  start_date: null,
  target_date: null,
  completed_at: null,
  sort_order: 1001,
  parent_task_id: null,
  parent_task_sort_order: null,
  extension_metadata: null,
  creator_user_id: null,
  created_at: "2026-10-28T00:00:00Z",
  updated_at: "2026-10-28T00:00:00Z",
};
const project = {
  projectId: "project-1",
  tasks: [task],
  statuses: ["todo", "done"].map((id, index) => ({
    id,
    project_id: "project-1",
    name: id === "todo" ? "Todo" : "Done",
    color: "220 16% 56%",
    sort_order: index,
    hidden: false,
    created_at: "2026-10-28T00:00:00Z",
  })),
  tags: [],
  taskTags: [],
  getTask: (id: string) => (id === task.id ? task : undefined),
};
const navigation = {
  goToProject: noop,
  goToProjectTask: noop,
  goToProjectTaskWorkspace: (...args: string[]) => {
    document.documentElement.dataset.openedExecution = JSON.stringify(args);
  },
  goToProjectWorkflowRun: noop,
  goToProjectWorkflowEdit: noop,
  goToProjectTaskArena: noop,
};
const actions = { executeAction: noop };
const deletion = { deleteSession: noop, pendingSessionId: null };

export const Actions = { DeleteTask: "DeleteTask" };
export const useProjectContext = () => project;
export const useActions = () => actions;
export const useAppNavigation = () => navigation;
export const useDeleteExecutionSession = () => deletion;
export const useCurrentKanbanRouteState = () => ({
  hostId: new URLSearchParams(location.search).get("host"),
  taskId: null,
  workspaceId: null,
  isWorkspaceCreateMode: false,
});

export const executionDataApi = {
  async listExecutions(options: ExecutionPageOptions) {
    const params = new URLSearchParams({
      project_id: options.projectId,
      host: options.hostId ?? "local",
    });
    if (options.cursor) params.set("cursor_id", options.cursor.id);
    const response = await fetch(`/__fixture/executions?${params}`, {
      signal: options.signal,
    });
    if (!response.ok) throw new Error("Fixture execution source unavailable");
    return response.json();
  },
};

export const bulkUpdateTasks = noop;
export const ProjectWorkspaceDefaultContext = () => null;
export const ProjectRightSidebarContainer = () => null;
export const TaskFloatingPanelContainer = () => null;
export const ProjectBoardActions = () => null;
