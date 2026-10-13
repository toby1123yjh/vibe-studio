import type {
  WorkflowCanvasEdgeState,
  WorkflowCanvasNodeState,
} from '../model/workflowCanvasVisualState';
import type { WorkflowNodeKind } from '../model/workflowGraph';

export const WORKFLOW_CANVAS_TOKEN_GROUPS = [
  'canvas',
  'node',
  'edge',
  'panel',
  'motion',
] as const;

export const WORKFLOW_CANVAS_CLASS_NAMES = {
  root: 'workflow-canvas-shell workflow-canvas-surface relative h-full w-full bg-[var(--workflow-canvas-bg)]',
  reactFlow:
    'workflow-canvas workflow-canvas-product bg-[var(--workflow-canvas-bg)]',
  controls:
    'workflow-canvas-controls rounded-lg border border-[var(--workflow-canvas-control-border)] bg-[var(--workflow-canvas-control-bg)] text-high shadow-[var(--workflow-canvas-control-shadow)] backdrop-blur',
  sidePanel:
    'workflow-side-panel-surface relative z-10 min-w-0 overflow-hidden border-l border-[var(--workflow-panel-border)] bg-[var(--workflow-panel-bg)] shadow-[var(--workflow-panel-shadow)]',
} as const;

export const WORKFLOW_CANVAS_COLOR_TOKENS = {
  grid: 'var(--workflow-canvas-grid)',
  brand: 'hsl(var(--brand))',
} as const;

export const WORKFLOW_CANVAS_NODE_SURFACE_CLASSES = {
  card: 'workflow-node-card rounded-[6px] border bg-[var(--workflow-node-bg)] shadow-[var(--workflow-node-shadow-base)] transition-colors duration-[140ms] motion-reduce:transition-none',
  structural:
    'bg-[var(--workflow-node-structural-bg)] shadow-[var(--workflow-node-shadow-structural)]',
  agent:
    'bg-[var(--workflow-node-bg)] shadow-[var(--workflow-node-shadow-base)]',
  agentHover: 'hover:shadow-[var(--workflow-node-shadow-hover)]',
  toolbar:
    'workflow-node-toolbar nodrag nopan absolute -top-9 right-2 z-20 flex items-center gap-1 rounded-lg border border-[var(--workflow-popover-border)] bg-[var(--workflow-popover-bg)] p-1 shadow-[var(--workflow-popover-shadow)] backdrop-blur transition-[opacity,transform,border-color,background-color,color,box-shadow] duration-150',
  actionButton:
    'border-[var(--workflow-popover-border)] bg-[var(--workflow-node-action-bg)] hover:border-brand/60 hover:bg-brand/15 hover:text-brand',
  handle:
    'h-4 w-4 border-[3px] border-[var(--workflow-node-port-ring)] bg-brand/80 shadow-[var(--workflow-node-shadow-port)] transition-colors hover:bg-brand',
  issueBadgeBorder: 'border-[var(--workflow-node-issue-border)]',
  staleBadge:
    'absolute -top-2 right-2 z-10 max-w-[170px] truncate rounded-full border border-amber-400/40 bg-amber-400/10 px-2 py-0.5 text-xs font-semibold text-warning shadow-[var(--workflow-node-shadow-stale)]',
  chip: 'inline-flex items-center rounded border border-secondary bg-secondary/30 px-1.5 py-0.5 text-xs font-medium leading-none text-low',
  chipTruncate:
    'inline-flex max-w-full items-center truncate rounded border border-secondary bg-secondary/30 px-1.5 py-0.5 text-xs font-medium leading-none text-low',
  note: 'relative h-full min-h-[120px] w-full min-w-[220px] overflow-hidden rounded-lg border p-3 shadow-[var(--workflow-note-shadow)] backdrop-blur transition-colors',
  stageGroup:
    'relative h-full min-h-[170px] w-full min-w-[360px] rounded-xl border p-4 text-low transition-colors',
} as const;

export const WORKFLOW_CANVAS_NODE_STATE_FRAME_CLASSES: Record<
  WorkflowCanvasNodeState,
  string
> = {
  draft: 'border-secondary',
  configured: 'border-secondary',
  pending: 'border-secondary',
  running: 'border-brand/70 shadow-[var(--workflow-node-shadow-running)]',
  succeeded: 'border-success/45 shadow-[var(--workflow-node-shadow-succeeded)]',
  failed: 'border-error/70 shadow-[var(--workflow-node-shadow-failed)]',
  waiting: 'border-warning/60 shadow-[var(--workflow-node-shadow-waiting)]',
  reused: 'border-brand/35',
  skipped: 'border-secondary opacity-80',
};

export const WORKFLOW_RUN_NODE_STATE_FRAME_CLASSES: Record<
  WorkflowCanvasNodeState,
  string
> = {
  draft: 'border-secondary bg-panel text-low',
  configured: 'border-secondary bg-panel text-low',
  pending: 'border-secondary bg-panel text-low',
  running:
    'border-brand/70 bg-brand/10 text-high shadow-[var(--workflow-node-shadow-running)]',
  succeeded:
    'border-success/45 bg-success/10 text-high shadow-[var(--workflow-node-shadow-succeeded)]',
  failed:
    'border-error/70 bg-error/10 text-high shadow-[var(--workflow-node-shadow-failed)]',
  waiting:
    'border-warning/60 bg-warning/10 text-high shadow-[var(--workflow-node-shadow-waiting)]',
  reused: 'border-brand/35 bg-panel text-high',
  skipped: 'border-secondary bg-panel text-low opacity-80',
};

export const WORKFLOW_CANVAS_NODE_STATE_CHIP_CLASSES: Record<
  WorkflowCanvasNodeState,
  string
> = {
  draft: 'border-secondary bg-secondary/30 text-low',
  configured: 'border-secondary bg-secondary/30 text-low',
  pending: 'border-secondary bg-secondary/30 text-low',
  running: 'border-brand/35 bg-brand/10 text-brand',
  succeeded: 'border-success/35 bg-success/10 text-success',
  failed: 'border-error/35 bg-error/10 text-error',
  waiting: 'border-warning/35 bg-warning/10 text-warning',
  reused: 'border-brand/25 bg-brand/5 text-brand',
  skipped: 'border-secondary bg-secondary/30 text-low',
};

export const WORKFLOW_RUN_NODE_STATE_CHIP_CLASSES: Record<
  WorkflowCanvasNodeState,
  string
> = {
  draft: 'border-secondary bg-panel text-low',
  configured: 'border-secondary bg-panel text-low',
  pending: 'border-secondary bg-panel text-low',
  running: 'border-brand/35 bg-brand/10 text-brand',
  succeeded: 'border-success/35 bg-success/10 text-success',
  failed: 'border-error/35 bg-error/10 text-error',
  waiting: 'border-warning/35 bg-warning/10 text-warning',
  reused: 'border-brand/25 bg-panel text-brand',
  skipped: 'border-secondary bg-panel text-low',
};

export const WORKFLOW_CANVAS_NODE_STATE_DOT_CLASSES: Record<
  WorkflowCanvasNodeState,
  string
> = {
  draft: 'bg-low/60',
  configured: 'bg-low',
  pending: 'bg-low',
  running: 'bg-brand shadow-[var(--workflow-node-shadow-dot-running)]',
  succeeded: 'bg-success shadow-[var(--workflow-node-shadow-dot-succeeded)]',
  failed: 'bg-error shadow-[var(--workflow-node-shadow-dot-failed)]',
  waiting: 'bg-warning shadow-[var(--workflow-node-shadow-dot-waiting)]',
  reused: 'bg-brand/60',
  skipped: 'bg-low/45',
};

export const WORKFLOW_CANVAS_EDGE_STATE_PATH_CLASSES: Record<
  WorkflowCanvasEdgeState,
  string
> = {
  idle: 'stroke-low/45',
  running: 'stroke-brand',
  succeeded: 'stroke-success/80',
  failed: 'stroke-error/85',
  waiting: 'stroke-warning/85',
  reused: 'stroke-brand/50',
  skipped: 'stroke-low/35',
};

export const WORKFLOW_CANVAS_EDGE_CLASSES = {
  actionButton:
    'workflow-edge-action-button nodrag nopan relative flex h-7 w-7 items-center justify-center rounded-[6px] border border-brand/35 bg-[var(--workflow-edge-action-bg)] text-brand shadow-[var(--workflow-edge-action-shadow)] transition-colors duration-150 hover:border-brand hover:bg-brand hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/55',
} as const;

export const WORKFLOW_CANVAS_NOTE_COLOR_CLASSES = {
  amber: 'border-amber-300/35 bg-amber-300/12 text-normal',
  blue: 'border-sky-300/30 bg-sky-300/10 text-normal',
  green: 'border-emerald-300/30 bg-emerald-300/10 text-normal',
  neutral: 'border-secondary bg-secondary/30 text-high',
} satisfies Record<string, string>;

export const WORKFLOW_CANVAS_GROUP_COLOR_CLASSES = {
  amber: 'border-amber-300/18 bg-amber-300/[0.035]',
  blue: 'border-sky-300/18 bg-sky-300/[0.035]',
  green: 'border-emerald-300/18 bg-emerald-300/[0.035]',
  neutral: 'border-secondary bg-white/[0.025]',
} satisfies Record<string, string>;

const WORKFLOW_NODE_STATUS_IDENTITY_OVERRIDE_STATES =
  new Set<WorkflowCanvasNodeState>([
    'pending',
    'running',
    'succeeded',
    'failed',
    'waiting',
    'reused',
    'skipped',
  ]);

export function getWorkflowNodeStatusClass(
  state: WorkflowCanvasNodeState
): string {
  return WORKFLOW_NODE_STATUS_IDENTITY_OVERRIDE_STATES.has(state)
    ? `node-status-${state}`
    : '';
}

export function getWorkflowNodeIdentityClass(
  nodeKind: WorkflowNodeKind | string,
  executor: string | null | undefined
): string {
  if (nodeKind === 'agent') {
    switch (executor) {
      case 'CLAUDE_CODE':
        return 'node-identity-claude';
      case 'CODEX':
        return 'node-identity-codex';
      case 'GEMINI':
        return 'node-identity-gemini';
      case 'OH_MY_PI':
        return 'node-identity-oh-my-pi';
      case 'OPENCODE':
        return 'node-identity-opencode';
      case 'DEEPSEEK_HARNESS':
        return 'node-identity-deepseek-harness';
      default:
        return 'node-identity-default';
    }
  }

  switch (nodeKind) {
    case 'task_splitter':
      return 'node-identity-task-splitter';
    case 'arena':
      return 'node-identity-arena';
    case 'condition':
      return 'node-identity-condition';
    case 'human_gate':
      return 'node-identity-human-gate';
    case 'start':
      return 'node-identity-start';
    case 'end':
      return 'node-identity-end';
    default:
      return 'node-identity-default';
  }
}
