import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from 'react';
import {
  closestCorners,
  defaultDropAnimationSideEffects,
  DndContext,
  DragOverlay,
  KeyboardSensor,
  pointerWithin,
  PointerSensor,
  useDroppable,
  useSensor,
  useSensors,
  type CollisionDetection,
  type DragEndEvent,
  type DragCancelEvent,
  type DragStartEvent,
  type DropAnimation,
  type KeyboardCoordinateGetter,
  type Modifier,
} from '@dnd-kit/core';
import {
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable';
import { CSS, getEventCoordinates } from '@dnd-kit/utilities';
import { useReducedMotion } from '@/shared/hooks/useReducedMotion';
import { Button } from '@vibe/ui/components/Button';
import { DegradedState, LoadingState } from '@vibe/ui/components/StateSurface';
import { useTranslation } from 'react-i18next';
import {
  ArrowRight,
  Circle,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  Clock3,
  GripVertical,
  LoaderCircle,
  MoreHorizontal,
  Plus,
  Search,
  Trash2,
  XCircle,
} from 'lucide-react';
import type { ExecutionStatus, ExecutionSummary } from 'shared/types';
import {
  KANBAN_POINTER_ACTIVATION_DISTANCE,
  findKanbanTask,
  isInteractiveDragTarget,
  moveKanbanTask,
  executionStatusLabel,
  type KanbanColumnProjection,
  type KanbanTaskProjection,
  type KanbanMoveUpdate,
} from '../model/project-kanban';
import './project-surfaces.css';
import {
  ExecutionDeleteButton,
  type ExecutionDeletionActions,
} from './ExecutionDeleteButton';

const kanbanCollisionDetection: CollisionDetection = (args) => {
  if (args.pointerCoordinates) return pointerWithin(args);

  const centerX = args.collisionRect.left + args.collisionRect.width / 2;
  const column = args.droppableContainers.find((container) => {
    const rect = args.droppableRects.get(container.id);
    return (
      container.data.current?.type === 'column' &&
      rect &&
      centerX >= rect.left &&
      centerX <= rect.right
    );
  });
  if (!column) return closestCorners(args);
  const tasks = args.droppableContainers.filter(
    (container) =>
      container.data.current?.type === 'task' &&
      container.data.current.statusId === column.data.current?.statusId
  );
  // A tall empty column's corners can be farther away than the source card's.
  // Choose the destination column first, then its cards (or the empty column).
  return closestCorners({
    ...args,
    droppableContainers: tasks.length ? tasks : [column],
  });
};

const kanbanKeyboardCoordinates: KeyboardCoordinateGetter = (event, args) => {
  if (event.code !== 'ArrowLeft' && event.code !== 'ArrowRight') {
    return sortableKeyboardCoordinates(event, args);
  }

  event.preventDefault();
  const { active, collisionRect, droppableContainers, droppableRects, over } =
    args.context;
  if (!active || !collisionRect) return undefined;

  const currentContainer = over
    ? droppableContainers.get(over.id)
    : droppableContainers.get(active.id);
  const currentStatusId = currentContainer?.data.current?.statusId;
  if (typeof currentStatusId !== 'string') return undefined;

  const columns = droppableContainers
    .getEnabled()
    .flatMap((container) => {
      const data = container.data.current;
      const rect = droppableRects.get(container.id);
      return data?.type === 'column' && rect
        ? [{ container, rect, statusId: String(data.statusId) }]
        : [];
    })
    .sort((left, right) => left.rect.left - right.rect.left);
  const currentColumnIndex = columns.findIndex(
    (column) => column.statusId === currentStatusId
  );
  const direction = event.code === 'ArrowRight' ? 1 : -1;
  const targetColumn = columns[currentColumnIndex + direction];
  if (!targetColumn) return undefined;

  const collisionCenter = collisionRect.top + collisionRect.height / 2;
  const closestTask = droppableContainers
    .getEnabled()
    .flatMap((container) => {
      const data = container.data.current;
      const rect = droppableRects.get(container.id);
      return data?.type === 'task' &&
        data.statusId === targetColumn.statusId &&
        rect
        ? [
            {
              rect,
              distance: Math.abs(rect.top + rect.height / 2 - collisionCenter),
            },
          ]
        : [];
    })
    .sort((left, right) => left.distance - right.distance)[0];
  const targetRect = closestTask?.rect ?? targetColumn.rect;
  return { x: targetRect.left, y: targetRect.top };
};

interface ProjectKanbanViewProps extends ExecutionDeletionActions {
  projectName: string;
  projectContext?: ReactNode;
  projectActions?: ReactNode;
  columns: KanbanColumnProjection[];
  taskCount: number;
  query: string;
  selectedTaskId: string | null;
  dragDisabled: boolean;
  projectSource?: {
    title: string;
    description?: string;
    retry(): void;
    retrying?: boolean;
  };
  executionSource: {
    state: 'ready' | 'loading' | 'degraded';
    title?: string;
    description?: string;
    retry?(): void;
    retrying?: boolean;
  };
  panel?: ReactNode;
  onQueryChange(query: string): void;
  onCreateTask(statusId?: string): void;
  onOpenTask(taskId: string, trigger: HTMLElement): void;
  onOpenExecution(execution: ExecutionSummary): void;
  onDeleteTask(taskId: string): Promise<void>;
  getExecutionUnavailableReason(execution: ExecutionSummary): string | null;
  onMove(updates: KanbanMoveUpdate[]): Promise<void>;
}

const STATUS_ICON: Record<ExecutionStatus, typeof Circle> = {
  draft: CircleDashed,
  pending: Clock3,
  running: LoaderCircle,
  waiting: Clock3,
  succeeded: CircleCheck,
  failed: CircleAlert,
  cancelled: XCircle,
};

function ExecutionStatusIcon({ status }: { status: ExecutionStatus }) {
  const Icon = STATUS_ICON[status];
  return (
    <Icon
      className={status === 'running' ? 'vk-task-status-icon--running' : ''}
      data-status={status}
      aria-label={executionStatusLabel(status)}
      size={14}
    />
  );
}

function TaskExecutionPreview({
  execution,
  onOpen,
  unavailableReason,
  preview = false,
  onDeleteExecution,
  deletingSessionId,
}: ExecutionDeletionActions & {
  execution: ExecutionSummary;
  onOpen(): void;
  unavailableReason: string | null;
  preview?: boolean;
}) {
  const content = (
    <>
      <ExecutionStatusIcon status={execution.status} />
      <span>{execution.title}</span>
      <ArrowRight aria-hidden="true" size={13} />
    </>
  );
  if (preview)
    return (
      <div className="vk-task-action-row">
        <div
          className="vk-kanban-task-preview"
          aria-disabled={unavailableReason ? true : undefined}
        >
          {content}
        </div>
        <ExecutionDeleteButton
          execution={execution}
          onDeleteExecution={onDeleteExecution}
          preview
        />
      </div>
    );
  return (
    <div className="vk-task-action-row" data-no-drag>
      <button
        type="button"
        className="vk-kanban-task-preview"
        data-no-drag
        aria-disabled={unavailableReason ? true : undefined}
        title={unavailableReason ?? undefined}
        onClick={(event) => {
          event.stopPropagation();
          if (!unavailableReason) onOpen();
        }}
      >
        {content}
      </button>
      <ExecutionDeleteButton
        execution={execution}
        onDeleteExecution={onDeleteExecution}
        deletingSessionId={deletingSessionId}
      />
    </div>
  );
}

function TaskCardContent({
  task,
  actions,
  preview = false,
  onOpenMore,
  onOpenExecution,
  onDeleteExecution,
  deletingSessionId,
  getExecutionUnavailableReason,
}: ExecutionDeletionActions & {
  task: KanbanTaskProjection;
  actions?: ReactNode;
  preview?: boolean;
  onOpenMore?: (trigger: HTMLElement) => void;
  onOpenExecution?: (execution: ExecutionSummary) => void;
  getExecutionUnavailableReason: (execution: ExecutionSummary) => string | null;
}) {
  return (
    <>
      <header className="vk-kanban-issue-card__meta">
        <span>{task.simpleId}</span>
        {actions}
      </header>
      <h3 title={task.title}>{task.title}</h3>
      <div className="vk-kanban-issue-card__labels">
        {task.priority ? (
          <span
            className="vk-priority"
            data-priority={task.priority}
            data-no-drag
          >
            {task.priority}
          </span>
        ) : null}
        {task.tags.map((tag) => (
          <span key={tag.id} className="vk-task-tag" data-no-drag>
            {tag.name}
          </span>
        ))}
      </div>
      {task.executions.length > 0 ? (
        <div className="vk-kanban-issue-card__tasks" data-no-drag>
          <small>{task.executions.length} executions</small>
          {task.executions.slice(0, 2).map((execution) => (
            <TaskExecutionPreview
              key={execution.id}
              execution={execution}
              preview={preview}
              onDeleteExecution={onDeleteExecution}
              deletingSessionId={deletingSessionId}
              onOpen={() => onOpenExecution?.(execution)}
              unavailableReason={getExecutionUnavailableReason(execution)}
            />
          ))}
          {task.executions.length > 2 ? (
            preview ? (
              <span className="vk-kanban-more-tasks">
                +{task.executions.length - 2} executions
              </span>
            ) : (
              <button
                type="button"
                className="vk-kanban-more-tasks"
                data-no-drag
                onClick={(event) => {
                  event.stopPropagation();
                  onOpenMore?.(event.currentTarget);
                }}
              >
                +{task.executions.length - 2} executions
              </button>
            )
          ) : null}
        </div>
      ) : null}
    </>
  );
}

interface KanbanTaskCardProps extends ExecutionDeletionActions {
  task: KanbanTaskProjection;
  selected: boolean;
  dragDisabled: boolean;
  onOpen(trigger: HTMLElement): void;
  onOpenExecution(execution: ExecutionSummary): void;
  onDelete(): Promise<void>;
  getExecutionUnavailableReason(execution: ExecutionSummary): string | null;
  placeholderHeight?: number;
}

function KanbanTaskCard({
  task,
  selected,
  dragDisabled,
  onOpen,
  onOpenExecution,
  onDeleteExecution,
  deletingSessionId,
  onDelete,
  getExecutionUnavailableReason,
  placeholderHeight,
}: KanbanTaskCardProps) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({
    id: task.id,
    data: { type: 'task', statusId: task.statusId },
    disabled: dragDisabled,
  });
  const pointerOrigin = useRef<{ x: number; y: number } | null>(null);
  const pointerMoved = useRef(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuTriggerRef = useRef<HTMLButtonElement>(null);
  const [menuOpen, setMenuOpen] = useState(false);

  useEffect(() => {
    if (!menuOpen) return;
    const closeOnOutsidePointer = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !menuRef.current?.contains(event.target) &&
        !menuTriggerRef.current?.contains(event.target)
      ) {
        setMenuOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      setMenuOpen(false);
      menuTriggerRef.current?.focus({ preventScroll: true });
    };
    document.addEventListener('pointerdown', closeOnOutsidePointer);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('pointerdown', closeOnOutsidePointer);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [menuOpen]);

  const handlePointerDown = (event: ReactPointerEvent<HTMLElement>) => {
    const target = event.target;
    const touchHandle =
      target instanceof Element
        ? target.closest('[data-touch-drag-handle]')
        : null;
    if (
      (event.pointerType === 'touch' && !touchHandle) ||
      (isInteractiveDragTarget(target, event.currentTarget) && !touchHandle)
    ) {
      return;
    }
    pointerOrigin.current = { x: event.clientX, y: event.clientY };
    pointerMoved.current = false;
    if (!dragDisabled) listeners?.onPointerDown?.(event);
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLElement>) => {
    if (!pointerOrigin.current) return;
    const distance = Math.hypot(
      event.clientX - pointerOrigin.current.x,
      event.clientY - pointerOrigin.current.y
    );
    if (distance >= KANBAN_POINTER_ACTIVATION_DISTANCE) {
      pointerMoved.current = true;
    }
  };

  const handleClick = (event: React.MouseEvent<HTMLElement>) => {
    if (
      isDragging ||
      pointerMoved.current ||
      isInteractiveDragTarget(event.target, event.currentTarget)
    ) {
      pointerMoved.current = false;
      return;
    }
    onOpen(event.currentTarget);
  };

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    if (
      event.target !== event.currentTarget &&
      isInteractiveDragTarget(event.target, event.currentTarget)
    ) {
      return;
    }
    if (event.key === 'Enter' && event.target === event.currentTarget) {
      event.preventDefault();
      onOpen(event.currentTarget);
      return;
    }
    if (!dragDisabled) listeners?.onKeyDown?.(event);
  };

  return (
    <article
      ref={setNodeRef}
      className="vk-kanban-issue-card"
      data-task-id={task.id}
      data-selected={selected}
      data-dragging={isDragging}
      data-drag-disabled={dragDisabled}
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
        height: placeholderHeight,
      }}
      {...attributes}
      onPointerDown={handlePointerDown}
      onPointerMoveCapture={handlePointerMove}
      onPointerUpCapture={() => {
        pointerOrigin.current = null;
      }}
      onKeyDown={handleKeyDown}
      onClick={handleClick}
      aria-label={`${task.simpleId}: ${task.title}. ${
        dragDisabled ? '' : 'Press Space to move or '
      }press Enter to open.`}
    >
      <TaskCardContent
        task={task}
        onOpenMore={onOpen}
        onOpenExecution={onOpenExecution}
        onDeleteExecution={onDeleteExecution}
        deletingSessionId={deletingSessionId}
        getExecutionUnavailableReason={getExecutionUnavailableReason}
        actions={
          <>
            <button
              ref={menuTriggerRef}
              type="button"
              className="vk-kanban-issue-card__menu"
              data-no-drag
              aria-label={`More actions for ${task.simpleId}`}
              aria-haspopup="menu"
              aria-expanded={menuOpen}
              onClick={(event) => {
                event.stopPropagation();
                setMenuOpen((open) => !open);
              }}
            >
              <MoreHorizontal aria-hidden="true" size={16} />
            </button>
            <button
              type="button"
              className="vk-kanban-card-drag-handle"
              data-touch-drag-handle
              aria-label={`Drag ${task.simpleId}`}
              title="Drag task"
            >
              <GripVertical aria-hidden="true" size={16} />
            </button>
            {menuOpen ? (
              <div
                ref={menuRef}
                className="vk-kanban-issue-card__menu-popover"
                role="menu"
              >
                <button
                  type="button"
                  role="menuitem"
                  data-no-drag
                  onClick={(event) => {
                    event.stopPropagation();
                    setMenuOpen(false);
                    void onDelete();
                  }}
                >
                  <Trash2 aria-hidden="true" size={15} />
                  Delete task
                </button>
              </div>
            ) : null}
          </>
        }
      />
    </article>
  );
}

function KanbanColumn({
  column,
  selectedTaskId,
  dragDisabled,
  onCreateTask,
  onOpenTask,
  onOpenExecution,
  onDeleteExecution,
  deletingSessionId,
  onDeleteTask,
  getExecutionUnavailableReason,
  dragSnapshot,
}: ExecutionDeletionActions & {
  column: KanbanColumnProjection;
  selectedTaskId: string | null;
  dragDisabled: boolean;
  onCreateTask(): void;
  onOpenTask(taskId: string, trigger: HTMLElement): void;
  onOpenExecution(execution: ExecutionSummary): void;
  onDeleteTask(taskId: string): Promise<void>;
  getExecutionUnavailableReason(execution: ExecutionSummary): string | null;
  dragSnapshot: KanbanDragSnapshot | null;
}) {
  const { setNodeRef, isOver } = useDroppable({
    id: column.id,
    data: { type: 'column', statusId: column.id },
  });
  const { t } = useTranslation('common');
  return (
    <section
      ref={setNodeRef}
      className="vk-kanban-column"
      data-over={isOver}
      aria-labelledby={`kanban-column-${column.id}`}
    >
      <header className="vk-kanban-column__header">
        <span
          className="vk-kanban-column__dot"
          style={
            { '--vk-status-color': `hsl(${column.color})` } as CSSProperties
          }
          aria-hidden="true"
        />
        <h2 id={`kanban-column-${column.id}`}>{column.name}</h2>
        <span className="vk-kanban-column__count">{column.tasks.length}</span>
        <button
          type="button"
          onClick={onCreateTask}
          aria-label={
            column.name.trim().toLowerCase() === 'discuss'
              ? t('projectSessions.create', { defaultValue: 'New discussion' })
              : `Create task in ${column.name}`
          }
        >
          <Plus aria-hidden="true" size={16} />
        </button>
      </header>
      <SortableContext
        items={column.tasks.map((task) => task.id)}
        strategy={verticalListSortingStrategy}
      >
        <div className="vk-kanban-column__cards">
          {column.tasks.map((task) => (
            <KanbanTaskCard
              key={task.id}
              task={
                dragSnapshot?.task.id === task.id ? dragSnapshot.task : task
              }
              placeholderHeight={
                dragSnapshot?.task.id === task.id
                  ? dragSnapshot.height
                  : undefined
              }
              selected={task.id === selectedTaskId}
              dragDisabled={dragDisabled}
              onOpen={(trigger) => onOpenTask(task.id, trigger)}
              onOpenExecution={onOpenExecution}
              onDeleteExecution={onDeleteExecution}
              deletingSessionId={deletingSessionId}
              onDelete={() => onDeleteTask(task.id)}
              getExecutionUnavailableReason={getExecutionUnavailableReason}
            />
          ))}
        </div>
      </SortableContext>
    </section>
  );
}

interface KanbanDragSnapshot {
  task: KanbanTaskProjection;
  width: number;
  height: number;
  previewHeight: number;
  gripOffsetY: number;
}

export function ProjectKanbanView({
  projectName,
  projectContext,
  projectActions,
  columns,
  taskCount,
  query,
  selectedTaskId,
  dragDisabled,
  projectSource,
  executionSource,
  panel,
  onQueryChange,
  onCreateTask,
  onOpenTask,
  onOpenExecution,
  onDeleteExecution,
  deletingSessionId,
  onDeleteTask,
  getExecutionUnavailableReason,
  onMove,
}: ProjectKanbanViewProps) {
  const { t } = useTranslation('common');
  const [displayColumns, setDisplayColumns] = useState(columns);
  const [activeTaskId, setActiveTaskId] = useState<string | null>(null);
  const [dragSnapshot, setDragSnapshot] = useState<KanbanDragSnapshot | null>(
    null
  );
  const reducedMotion = useReducedMotion();
  const dropCleanupRef = useRef<(() => void) | null>(null);
  useEffect(() => () => dropCleanupRef.current?.(), []);
  const previewModifiers = useMemo<Modifier[]>(
    () => [
      ({ transform }) => ({
        ...transform,
        y: transform.y + (dragSnapshot?.gripOffsetY ?? 0),
      }),
    ],
    [dragSnapshot]
  );
  const dropAnimation = useMemo<DropAnimation | null>(
    () =>
      reducedMotion
        ? null
        : {
            duration: 160,
            easing: 'ease-out',
            keyframes: ({ transform }) => [
              {
                transform: CSS.Transform.toString({
                  ...transform.initial,
                  scaleX: 1,
                  scaleY: 1,
                }),
              },
              {
                transform: CSS.Transform.toString({
                  ...transform.final,
                  scaleX: 1,
                  scaleY: 1,
                }),
              },
            ],
            sideEffects: (parameters) => {
              // A second drag can start before the previous return finishes.
              // Restore before capturing styles, and make old completion inert.
              dropCleanupRef.current?.();
              const restore = defaultDropAnimationSideEffects({
                styles: { active: { opacity: '0' } },
              })(parameters);
              let restored = false;
              const cleanup = () => {
                if (restored) return;
                restored = true;
                restore?.();
                if (dropCleanupRef.current === cleanup) {
                  dropCleanupRef.current = null;
                }
              };
              dropCleanupRef.current = cleanup;
              return cleanup;
            },
          },
    [reducedMotion]
  );
  const [announcement, setAnnouncement] = useState('');
  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: { distance: KANBAN_POINTER_ACTIVATION_DISTANCE },
    }),
    useSensor(KeyboardSensor, { coordinateGetter: kanbanKeyboardCoordinates })
  );
  const sourceStates = [
    projectSource
      ? {
          id: 'project',
          state: 'degraded' as const,
          ...projectSource,
        }
      : null,
    executionSource.state === 'ready'
      ? null
      : { id: 'tasks', ...executionSource },
  ].filter((source) => source !== null);

  useEffect(() => setDisplayColumns(columns), [columns]);

  const activeTask = useMemo(
    () => (activeTaskId ? findKanbanTask(displayColumns, activeTaskId) : null),
    [activeTaskId, displayColumns]
  );

  const handleDragStart = (event: DragStartEvent) => {
    dropCleanupRef.current?.();
    const task = findKanbanTask(displayColumns, String(event.active.id));
    const source = event.activatorEvent.target;
    const rect =
      event.active.rect.current.initial ??
      (source instanceof Element
        ? source.closest('[data-task-id]')?.getBoundingClientRect()
        : undefined);
    setDragSnapshot(null);
    if (task && rect) {
      const previewHeight = Math.min(
        rect.height,
        Math.min(480, window.innerHeight * 0.65)
      );
      const pointer = getEventCoordinates(event.activatorEvent);
      const gripY = pointer ? pointer.y - rect.top : 0;
      setDragSnapshot({
        task: structuredClone(task),
        width: rect.width,
        height: rect.height,
        previewHeight,
        gripOffsetY:
          previewHeight < rect.height
            ? Math.max(0, gripY - previewHeight + 24)
            : 0,
      });
    }
    setActiveTaskId(task?.id ?? null);
    if (task) setAnnouncement(`Picked up ${task.simpleId}.`);
  };

  const handleDragCancel = (event?: DragCancelEvent) => {
    const cancelledTask = event
      ? findKanbanTask(displayColumns, String(event.active.id))
      : activeTask;
    if (cancelledTask) {
      setAnnouncement(`Movement cancelled for ${cancelledTask.simpleId}.`);
    }
    setActiveTaskId(null);
  };

  const handleDragEnd = async (event: DragEndEvent) => {
    const task = findKanbanTask(displayColumns, String(event.active.id));
    const overId = event.over ? String(event.over.id) : null;
    if (!task || !overId) {
      handleDragCancel();
      return;
    }

    const targetTask = findKanbanTask(displayColumns, overId);
    if (targetTask?.id === task.id) {
      setAnnouncement(`No valid destination for ${task.simpleId}.`);
      setActiveTaskId(null);
      return;
    }
    const targetColumn = targetTask
      ? displayColumns.find((column) => column.id === targetTask.statusId)
      : displayColumns.find((column) => column.id === overId);
    if (!targetColumn) {
      setAnnouncement(`No valid destination for ${task.simpleId}.`);
      setActiveTaskId(null);
      return;
    }

    const targetIndex = targetTask
      ? targetColumn.tasks.findIndex(
          (candidate) => candidate.id === targetTask.id
        )
      : targetColumn.tasks.length;
    const move = moveKanbanTask(displayColumns, {
      taskId: task.id,
      sourceStatusId: task.statusId,
      targetStatusId: targetColumn.id,
      targetIndex,
    });
    setActiveTaskId(null);
    if (!move) {
      setAnnouncement(`No valid destination for ${task.simpleId}.`);
      return;
    }

    const previousColumns = displayColumns;
    setDisplayColumns(move.columns);
    setAnnouncement(`Moved ${task.simpleId} to ${targetColumn.name}.`);
    try {
      await onMove(move.updates);
    } catch {
      setDisplayColumns(previousColumns);
      setAnnouncement(
        `Move failed. ${task.simpleId} was returned to its previous position.`
      );
    }
  };

  return (
    <section className="vk-project-kanban" aria-label={`${projectName} board`}>
      <header className="vk-project-kanban__toolbar">
        <div className="vk-project-kanban__identity">
          <span aria-hidden="true">
            {projectName.slice(0, 1).toLocaleUpperCase()}
          </span>
          <div className="vk-project-kanban__identity-content">
            <strong title={projectName}>{projectName}</strong>
            {projectContext}
          </div>
        </div>
        {projectActions}
        <label className="vk-kanban-search">
          <Search aria-hidden="true" size={16} />
          <span className="vk-visually-hidden">
            Search tasks in {projectName}
          </span>
          <input
            type="search"
            value={query}
            onChange={(event) => onQueryChange(event.target.value)}
            placeholder="Search tasks"
          />
        </label>
        <span className="vk-project-kanban__issue-count">
          {taskCount} {taskCount === 1 ? 'Task' : 'Tasks'}
        </span>
        <button
          type="button"
          className="vk-primary-action"
          onClick={() => onCreateTask()}
        >
          <Plus aria-hidden="true" size={16} />
          New task
        </button>
      </header>

      {sourceStates.length > 0 ? (
        <div className="vk-project-kanban__task-source !flex-col !items-stretch !gap-0 !p-0">
          {sourceStates.map((source) => {
            const SourceState =
              source.state === 'loading' ? LoadingState : DegradedState;
            return (
              <SourceState
                key={source.id}
                compact
                className="w-full !flex-row !justify-start !rounded-none !text-left"
                title={source.title ?? 'Loading executions…'}
                description={source.description}
                action={
                  source.retry ? (
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      className="min-h-11"
                      loading={source.retrying}
                      loadingLabel={t('buttons.retry')}
                      onClick={source.retry}
                    >
                      {t('buttons.retry')}
                    </Button>
                  ) : undefined
                }
              />
            );
          })}
        </div>
      ) : null}

      <div className="vk-kanban-dnd-root">
        <DndContext
          sensors={sensors}
          collisionDetection={kanbanCollisionDetection}
          onDragStart={handleDragStart}
          onDragCancel={handleDragCancel}
          onDragEnd={(event) => void handleDragEnd(event)}
          autoScroll={{ enabled: true, threshold: { x: 0.12, y: 0.12 } }}
        >
          <div
            className="vk-kanban-scroll"
            tabIndex={0}
            aria-label="Kanban columns"
          >
            <div className="vk-kanban-columns">
              {displayColumns.map((column) => (
                <KanbanColumn
                  key={column.id}
                  column={column}
                  selectedTaskId={selectedTaskId}
                  dragDisabled={dragDisabled}
                  onCreateTask={() => onCreateTask(column.id)}
                  onOpenTask={onOpenTask}
                  onOpenExecution={onOpenExecution}
                  onDeleteExecution={onDeleteExecution}
                  deletingSessionId={deletingSessionId}
                  onDeleteTask={onDeleteTask}
                  getExecutionUnavailableReason={getExecutionUnavailableReason}
                  dragSnapshot={activeTaskId ? dragSnapshot : null}
                />
              ))}
            </div>
          </div>
          <DragOverlay
            adjustScale={false}
            transition={reducedMotion ? 'none' : undefined}
            modifiers={previewModifiers}
            dropAnimation={dropAnimation}
          >
            {activeTaskId && dragSnapshot ? (
              <div
                className="vk-kanban-issue-card vk-kanban-drag-preview"
                data-clipped={dragSnapshot.previewHeight < dragSnapshot.height}
                aria-hidden="true"
                style={{
                  width: dragSnapshot.width,
                  height: dragSnapshot.previewHeight,
                }}
              >
                <TaskCardContent
                  task={dragSnapshot.task}
                  preview
                  onDeleteExecution={onDeleteExecution}
                  getExecutionUnavailableReason={getExecutionUnavailableReason}
                  actions={
                    <>
                      <div className="vk-kanban-issue-card__menu">
                        <MoreHorizontal aria-hidden="true" size={16} />
                      </div>
                      <div className="vk-kanban-card-drag-handle">
                        <GripVertical aria-hidden="true" size={16} />
                      </div>
                    </>
                  }
                />
              </div>
            ) : null}
          </DragOverlay>
        </DndContext>
      </div>

      <p className="vk-visually-hidden" role="status" aria-live="polite">
        {announcement}
      </p>
      {panel}
    </section>
  );
}
