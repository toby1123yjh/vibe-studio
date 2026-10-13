import type { TFunction } from 'i18next';
import type {
  WorkflowEdgeKind,
  WorkflowNodeData,
  WorkflowNodeKind,
} from './workflowGraph';

const NODE_KIND_LABELS: Record<WorkflowNodeKind, string> = {
  start: 'Start',
  end: 'End',
  agent: 'Agent',
  task_splitter: 'Task Splitter',
  condition: 'Condition',
  human_gate: 'Human Gate',
  transform: 'Transform',
  arena: 'Arena',
};

const NODE_KIND_I18N_KEYS: Record<WorkflowNodeKind, string> = {
  start: 'workflow.nodes.start.label',
  end: 'workflow.nodes.end.label',
  agent: 'workflow.nodes.agent.label',
  task_splitter: 'workflow.nodes.taskSplitter.label',
  condition: 'workflow.nodes.condition.label',
  human_gate: 'workflow.nodes.humanGate.label',
  transform: 'workflow.nodes.transform.label',
  arena: 'workflow.nodes.arena.label',
};

const EDGE_LABELS: Record<Exclude<WorkflowEdgeKind, 'default'>, string> = {
  condition_branch: 'Condition',
  approval: 'Approve',
  rejection: 'Reject',
  arena_winner: 'Winner',
};

const EDGE_LABEL_I18N_KEYS: Record<
  Exclude<WorkflowEdgeKind, 'default'>,
  string
> = {
  condition_branch: 'workflow.edges.condition',
  approval: 'workflow.edges.approve',
  rejection: 'workflow.edges.reject',
  arena_winner: 'workflow.edges.winner',
};

function translate(
  t: TFunction<'common'> | undefined,
  key: string,
  fallback: string,
  options?: Record<string, unknown>
): string {
  return t ? t(key, { defaultValue: fallback, ...options }) : fallback;
}

export interface WorkflowNodeVisual {
  accentClass: string;
  iconClass: string;
  badgeClass: string;
}

export interface WorkflowEdgeVisual {
  label?: string;
  pathClass: string;
  chipClass: string;
}

export interface WorkflowNodeMetadataChip {
  label: string;
  value: string;
}

export interface WorkflowNodeRouteHint {
  label: string;
  tone: 'brand' | 'success' | 'warning' | 'danger';
}

const NODE_VISUALS: Record<WorkflowNodeKind, WorkflowNodeVisual> = {
  start: {
    accentClass: 'bg-emerald-500',
    iconClass: 'bg-emerald-500/10 text-emerald-300',
    badgeClass: 'border-emerald-500/30 bg-emerald-500/10 text-emerald-300',
  },
  end: {
    accentClass: 'bg-slate-500',
    iconClass: 'bg-slate-500/10 text-slate-300',
    badgeClass: 'border-slate-500/30 bg-slate-500/10 text-slate-300',
  },
  agent: {
    accentClass: 'bg-sky-500',
    iconClass: 'bg-sky-500/10 text-sky-300',
    badgeClass: 'border-sky-500/30 bg-sky-500/10 text-sky-300',
  },
  task_splitter: {
    accentClass: 'bg-orange-500',
    iconClass: 'bg-orange-500/10 text-orange-300',
    badgeClass: 'border-orange-500/30 bg-orange-500/10 text-orange-300',
  },
  condition: {
    accentClass: 'bg-amber-500',
    iconClass: 'bg-amber-500/10 text-amber-300',
    badgeClass: 'border-amber-500/30 bg-amber-500/10 text-amber-300',
  },
  human_gate: {
    accentClass: 'bg-violet-500',
    iconClass: 'bg-violet-500/10 text-violet-300',
    badgeClass: 'border-violet-500/30 bg-violet-500/10 text-violet-300',
  },
  transform: {
    accentClass: 'bg-cyan-500',
    iconClass: 'bg-cyan-500/10 text-cyan-300',
    badgeClass: 'border-cyan-500/30 bg-cyan-500/10 text-cyan-300',
  },
  arena: {
    accentClass: 'bg-fuchsia-500',
    iconClass: 'bg-fuchsia-500/10 text-fuchsia-300',
    badgeClass: 'border-fuchsia-500/30 bg-fuchsia-500/10 text-fuchsia-300',
  },
};

const EDGE_VISUALS: Record<WorkflowEdgeKind, WorkflowEdgeVisual> = {
  default: {
    pathClass: 'stroke-low/60',
    chipClass: 'border-secondary bg-panel text-low',
  },
  condition_branch: {
    label: EDGE_LABELS.condition_branch,
    pathClass: 'stroke-amber-500',
    chipClass: 'border-amber-500/30 bg-amber-500/10 text-amber-700',
  },
  approval: {
    label: EDGE_LABELS.approval,
    pathClass: 'stroke-emerald-500',
    chipClass: 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700',
  },
  rejection: {
    label: EDGE_LABELS.rejection,
    pathClass: 'stroke-rose-500',
    chipClass: 'border-rose-500/30 bg-rose-500/10 text-rose-700',
  },
  arena_winner: {
    label: EDGE_LABELS.arena_winner,
    pathClass: 'stroke-fuchsia-500',
    chipClass: 'border-fuchsia-500/30 bg-fuchsia-500/10 text-fuchsia-700',
  },
};

export interface WorkflowEdgeKindOption {
  value: WorkflowEdgeKind;
  label: string;
  description: string;
}

const EDGE_KIND_OPTIONS: WorkflowEdgeKindOption[] = [
  {
    value: 'default',
    label: 'Default',
    description: 'Continue to the next node.',
  },
  {
    value: 'condition_branch',
    label: 'Condition',
    description: 'Route through a conditional branch.',
  },
  {
    value: 'approval',
    label: 'Approve',
    description: 'Continue after a human approval.',
  },
  {
    value: 'rejection',
    label: 'Reject',
    description: 'Route after a human rejection.',
  },
  {
    value: 'arena_winner',
    label: 'Winner',
    description: 'Promote the winning arena attempt.',
  },
];

export function getWorkflowNodeKindLabel(
  kind: WorkflowNodeKind,
  t?: TFunction<'common'>
): string {
  return translate(t, NODE_KIND_I18N_KEYS[kind], NODE_KIND_LABELS[kind]);
}

export function getWorkflowNodeSummary(
  kind: WorkflowNodeKind,
  data: WorkflowNodeData,
  t?: TFunction<'common'>
): string {
  switch (kind) {
    case 'start':
      return translate(t, 'workflow.nodes.start.summary', 'Entry point');
    case 'end':
      return translate(t, 'workflow.nodes.end.summary', 'Exit point');
    case 'agent':
      return translate(t, 'workflow.nodes.agent.summary', 'Role: {{role}}', {
        role: String(data.role_template_id || 'custom'),
      });
    case 'task_splitter':
      return translate(
        t,
        'workflow.nodes.taskSplitter.summary',
        'Creates file-backed subtasks'
      );
    case 'condition':
      return translate(
        t,
        'workflow.nodes.condition.summary',
        '{{count}} branches',
        { count: Array.isArray(data.branches) ? data.branches.length : 0 }
      );
    case 'human_gate':
      return translate(
        t,
        'workflow.nodes.humanGate.summary',
        'Waits for approval'
      );
    case 'transform':
      return translate(
        t,
        'workflow.nodes.transform.summary',
        'Mode: {{mode}}',
        {
          mode: String(data.mode || 'template'),
        }
      );
    case 'arena':
      return translate(
        t,
        'workflow.nodes.arena.summary',
        '{{count}} attempts',
        {
          count: Array.isArray(data.attempts) ? data.attempts.length : 0,
        }
      );
  }
}

export function getWorkflowNodeVisual(
  kind: WorkflowNodeKind
): WorkflowNodeVisual {
  return NODE_VISUALS[kind];
}

function formatToken(value: unknown, fallback: string): string {
  return String(value || fallback).replace(/_/g, ' ');
}

export function getWorkflowNodeMetadata(
  kind: WorkflowNodeKind,
  data: WorkflowNodeData,
  t?: TFunction<'common'>
): WorkflowNodeMetadataChip[] {
  switch (kind) {
    case 'start':
      return [
        {
          label: translate(t, 'workflow.metadata.trigger', 'Trigger'),
          value: 'manual',
        },
      ];
    case 'end':
      return [
        {
          label: translate(t, 'workflow.metadata.state', 'State'),
          value: 'terminal',
        },
      ];
    case 'agent':
      return [
        {
          label: translate(t, 'workflow.metadata.role', 'Role'),
          value: formatToken(data.role_template_id, 'custom'),
        },
      ];
    case 'task_splitter':
      return [
        {
          label: translate(t, 'workflow.split.output', 'Output'),
          value: 'tasks.json',
        },
      ];
    case 'condition':
      return [
        {
          label: translate(t, 'workflow.metadata.branchCount', 'Branches'),
          value: translate(
            t,
            'workflow.metadata.branchCountValue',
            '{{count}} branches',
            {
              count: Array.isArray(data.branches) ? data.branches.length : 0,
            }
          ),
        },
        {
          label: translate(t, 'workflow.metadata.routingMode', 'Routing'),
          value: formatToken(data.routing_mode, 'single'),
        },
      ];
    case 'human_gate':
      return [
        {
          label: translate(t, 'workflow.metadata.action', 'Action'),
          value: formatToken(data.required_action, 'approve or reject'),
        },
      ];
    case 'transform':
      return [
        {
          label: translate(t, 'workflow.metadata.mode', 'Mode'),
          value: formatToken(data.mode, 'template'),
        },
      ];
    case 'arena':
      return [
        {
          label: translate(t, 'workflow.metadata.attemptCount', 'Attempts'),
          value: translate(
            t,
            'workflow.metadata.attemptCountValue',
            '{{count}} attempts',
            {
              count: Array.isArray(data.attempts) ? data.attempts.length : 0,
            }
          ),
        },
        {
          label: translate(t, 'workflow.metadata.promote', 'Promote'),
          value: formatToken(data.promote_strategy, 'manual'),
        },
      ];
  }
}

export function getWorkflowNodeRouteHints(
  kind: WorkflowNodeKind,
  data: WorkflowNodeData,
  t?: TFunction<'common'>
): WorkflowNodeRouteHint[] {
  if (kind === 'condition') {
    return (data.branches ?? []).slice(0, 3).map((branch, index) => ({
      label:
        branch.condition?.trim() ||
        branch.target_node_id ||
        translate(t, 'workflow.metadata.branch', 'Branch {{index}}', {
          index: index + 1,
        }),
      tone: index === 0 ? 'success' : 'warning',
    }));
  }

  if (kind === 'human_gate') {
    return [
      {
        label: translate(t, 'workflow.edges.approve', 'Approve'),
        tone: 'success',
      },
      {
        label: translate(t, 'workflow.edges.reject', 'Reject'),
        tone: 'danger',
      },
    ];
  }

  if (kind === 'arena') {
    return [
      { label: translate(t, 'workflow.edges.winner', 'Winner'), tone: 'brand' },
    ];
  }

  return [];
}

export function getWorkflowEdgeLabel(
  kind: WorkflowEdgeKind,
  t?: TFunction<'common'>
): string | undefined {
  if (kind === 'default') {
    return undefined;
  }
  return translate(t, EDGE_LABEL_I18N_KEYS[kind], EDGE_LABELS[kind]);
}

export function getWorkflowEdgeVisual(
  kind: WorkflowEdgeKind,
  t?: TFunction<'common'>
): WorkflowEdgeVisual {
  const visual = EDGE_VISUALS[kind];
  if (kind === 'default') {
    return visual;
  }
  return {
    ...visual,
    label: getWorkflowEdgeLabel(kind, t),
  };
}

export function getWorkflowEdgeKindOptions(
  t?: TFunction<'common'>
): WorkflowEdgeKindOption[] {
  if (!t) return EDGE_KIND_OPTIONS;
  return EDGE_KIND_OPTIONS.map((option) => ({
    ...option,
    label:
      option.value === 'default'
        ? t('workflow.edges.default')
        : (getWorkflowEdgeLabel(option.value, t) ?? option.label),
    description: t(`workflow.edges.options.${option.value}`),
  }));
}
