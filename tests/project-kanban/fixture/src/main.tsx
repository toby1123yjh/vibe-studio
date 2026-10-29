import * as React from "react";
import { createRoot } from "react-dom/client";
import type { ExecutionSummary } from "shared/types";
import type {
  KanbanColumnProjection,
  KanbanMoveUpdate,
} from "../../../../packages/web-core/src/features/projects/model/project-kanban";
import { ProjectKanbanView } from "../../../../packages/web-core/src/features/projects/ui/ProjectKanbanView";
import { ProjectKanbanSkeleton } from "../../../../packages/web-core/src/features/projects/ui/ProjectKanbanSkeleton";
import i18n from "../../../../packages/web-core/src/i18n/config";
import "../../../../packages/ui/src/styles/tokens.css";
import "./style.css";

void i18n.changeLanguage("en");

const task: ExecutionSummary = {
  id: "task-1",
  project_id: "project-1",
  task_id: "issue-1",
  parent_execution_id: null,
  title: "Run the canonical agent task",
  execution_kind: "agent",
  status: "running",
  open_target: {
    kind: "agent",
    session_id: "session-1",
    workspace_id: "workspace-1",
  },
  created_at: "2026-08-29T00:00:00Z",
  updated_at: "2026-08-29T00:00:00Z",
};

const columns: KanbanColumnProjection[] = [
  {
    id: "todo",
    name: "Todo",
    color: "220 16% 56%",
    sortOrder: 1,
    tasks: [
      {
        id: "issue-1",
        simpleId: "VK-1",
        title: "Keyboard and pointer interaction",
        statusId: "todo",
        priority: "high",
        sortOrder: 1,
        tags: Array.from(
          { length: new URLSearchParams(location.search).has("tall") ? 48 : 1 },
          (_, index) => ({
            id: `tag-${index}`,
            project_id: "project-1",
            name: `Planning label ${index + 1}`,
            color: "211 90% 50%",
          }),
        ),
        executions: [task],
      },
      {
        id: "issue-2",
        simpleId: "VK-2",
        title: "Second sortable task",
        statusId: "todo",
        priority: null,
        sortOrder: 2,
        tags: [],
        executions: [],
      },
      ...Array.from({ length: 10 }, (_, index) => ({
        id: `issue-long-${index}`,
        simpleId: `VK-${index + 3}`,
        title: `Long column issue ${index + 1}`,
        statusId: "todo",
        priority: null,
        sortOrder: index + 3,
        tags: [],
        executions: [],
      })),
    ],
  },
  {
    id: "doing",
    name: "Doing",
    color: "211 90% 50%",
    sortOrder: 2,
    tasks: [
      {
        id: "issue-doing",
        simpleId: "VK-20",
        title: "Cross-column destination",
        statusId: "doing",
        priority: null,
        sortOrder: 1,
        tags: [],
        executions: [],
      },
    ],
  },
  {
    id: "done",
    name: "Done",
    color: "142 71% 45%",
    sortOrder: 3,
    tasks: [],
  },
];

function Harness() {
  const boardColumns = React.useMemo(
    () =>
      new URLSearchParams(location.search).has("discuss")
        ? [
            {
              id: "discuss",
              name: "Discuss",
              color: "215 12% 55%",
              sortOrder: 0,
              tasks: [],
            },
            ...columns,
          ]
        : columns,
    [],
  );
  const [selectedTaskId, setSelectedTaskId] = React.useState<string | null>(
    null,
  );
  const [moveCount, setMoveCount] = React.useState(0);
  const [taskOpenCount, setTaskOpenCount] = React.useState(0);
  const [deletedId, setDeletedId] = React.useState("");
  const [rejectMove, setRejectMove] = React.useState(false);
  const [lastMove, setLastMove] = React.useState<KanbanMoveUpdate[]>([]);
  const moveCountRef = React.useRef(0);
  const [loading, setLoading] = React.useState(
    new URLSearchParams(location.search).has("loading"),
  );

  const move = async (updates: KanbanMoveUpdate[]) => {
    setLastMove(updates);
    moveCountRef.current += 1;
    setMoveCount(moveCountRef.current);
    if (rejectMove) throw new Error("Fixture mutation failure");
  };

  return (
    <>
      {loading ? (
        <ProjectKanbanSkeleton projectName="Fixture project" />
      ) : (
        <ProjectKanbanView
          projectName="Fixture project"
          columns={boardColumns}
          taskCount={boardColumns.reduce(
            (count, column) => count + column.tasks.length,
            0,
          )}
          query=""
          selectedTaskId={selectedTaskId}
          dragDisabled={false}
          executionSource={{ state: "ready" }}
          panel={
            selectedTaskId ? (
              <aside
                className="vk-task-floating-panel"
                aria-label="Task details"
              >
                <div className="fixture-panel">
                  <h2>{selectedTaskId}</h2>
                  <button type="button" onClick={() => setSelectedTaskId(null)}>
                    Close panel
                  </button>
                </div>
              </aside>
            ) : null
          }
          onQueryChange={() => undefined}
          onCreateTask={() => undefined}
          onOpenTask={(taskId) => setSelectedTaskId(taskId)}
          onOpenExecution={() => setTaskOpenCount((count) => count + 1)}
          onDeleteTask={async (id) => {
            setDeletedId(id);
          }}
          getExecutionUnavailableReason={() => null}
          onMove={move}
        />
      )}
      <div className="fixture-controls">
        <button type="button" onClick={() => setLoading(false)}>
          Finish loading
        </button>
        <output data-testid="move-count">{moveCount}</output>
        <output data-testid="last-move" hidden>
          {JSON.stringify(lastMove)}
        </output>
        <output data-testid="task-open-count">{taskOpenCount}</output>
        <output data-testid="deleted-id">{deletedId}</output>
        <button type="button" onClick={() => setRejectMove((value) => !value)}>
          Toggle mutation failure
        </button>
      </div>
    </>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("Project Kanban fixture root is missing");
createRoot(root).render(<Harness />);
