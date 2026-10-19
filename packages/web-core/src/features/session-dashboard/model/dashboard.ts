import type { TFunction } from 'i18next';
import { isDefaultProject } from '@/shared/lib/defaultProject';
import type {
  DashboardChildRow,
  DashboardExpandedPage,
  DashboardFilters,
  DashboardNamedRef,
  DashboardOpenTarget,
  DashboardWorkRef,
  DashboardWorkRow,
} from 'shared/types';

export function dashboardProjectTitle(
  project: DashboardNamedRef | null | undefined,
  t: TFunction<'common'>
): string {
  if (!project) return t('sessionDashboard.noProject');
  return isDefaultProject(project.id)
    ? t('defaultProject.name')
    : (project.title ?? project.id.slice(0, 8));
}

export const DEFAULT_DASHBOARD_FILTERS: DashboardFilters = {
  status: 'current',
  kinds: [],
  project_id: null,
  provider_id: null,
  directory_kind: null,
  search: '',
  history_hours: 24,
  group_by_project: true,
};

export function dashboardWorkKey(work: DashboardWorkRef): string {
  return `${work.kind}:${work.id}`;
}

export function dashboardRowKey(
  hostId: string | null,
  work: DashboardWorkRef,
  child?: DashboardChildRow
): string {
  return JSON.stringify([hostId, work.kind, work.id, child?.id ?? null]);
}

export function dashboardQueryKey(
  scopeKey: string,
  filters: DashboardFilters,
  snapshotId: string | null,
  page: number,
  expanded: readonly DashboardExpandedPage[]
) {
  return [
    'session-dashboard',
    scopeKey,
    filters,
    snapshotId,
    page,
    expanded,
  ] as const;
}

/** Routes are built from validated identities, never from response-provided URLs. */
export function dashboardTargetHref(
  hostId: string | null,
  target: DashboardOpenTarget | null,
  owner?: Pick<DashboardWorkRow, 'project' | 'task'>
): string | null {
  if (!target) return null;
  const prefix = hostId ? `/hosts/${encodeURIComponent(hostId)}` : '';
  if (target.kind === 'session') {
    return `${prefix}/workspaces/${encodeURIComponent(target.workspace_id)}?session_id=${encodeURIComponent(target.session_id)}`;
  }
  if (target.kind === 'workflow') {
    const projectId = target.project_id ?? owner?.project?.id;
    if (!projectId) return null;
    return target.workflow_run_id
      ? `${prefix}/projects/${encodeURIComponent(projectId)}/workflow-runs/${encodeURIComponent(target.workflow_run_id)}`
      : `${prefix}/projects/${encodeURIComponent(projectId)}/workflows`;
  }
  const projectId = target.project_id ?? owner?.project?.id;
  if (!projectId || !owner?.task?.id) return null;
  return `${prefix}/projects/${encodeURIComponent(projectId)}/tasks/${encodeURIComponent(owner.task.id)}/arena/${encodeURIComponent(target.arena_group_id)}`;
}

export function dashboardConversationKey(
  scopeKey: string,
  target: Extract<DashboardOpenTarget, { kind: 'session' }>
): string {
  return JSON.stringify([
    scopeKey,
    target.workspace_id,
    target.session_id,
    target.workflow_run_id,
    target.node_execution_id,
    target.agent_run_id,
  ]);
}

export function reconcileDashboardExpanded(
  expanded: readonly DashboardExpandedPage[],
  rows: readonly DashboardWorkRow[]
): DashboardExpandedPage[] {
  const byKey = new Map(rows.map((row) => [dashboardWorkKey(row.work), row]));
  return expanded.flatMap((entry) => {
    const row = byKey.get(dashboardWorkKey(entry.work));
    if (!row || row.invalidated) return [];
    return [
      row.workflow_run_id !== entry.workflow_run_id
        ? {
            ...entry,
            workflow_run_id: row.workflow_run_id,
            snapshot_id: null,
            page: 0,
          }
        : entry,
    ];
  });
}

export function dashboardStatusTone(group: string): string {
  if (group === 'failed') return 'error';
  if (group === 'waiting') return 'warning';
  if (group === 'running') return 'active';
  if (group === 'finished') return 'complete';
  return 'neutral';
}

export function dashboardActivityKey(
  kind: string,
  status: string | null | undefined
): string {
  if (kind === 'tool') {
    const states = [
      'created',
      'running',
      'waiting_approval',
      'approved',
      'denied',
      'succeeded',
      'failed',
      'timed_out',
    ];
    return `sessionDashboard.toolStatuses.${status && states.includes(status) ? status : 'unknown'}`;
  }
  return `sessionDashboard.activities.${kind}`;
}

export function dashboardTimestamp(
  value: string | null | undefined
): number | null {
  if (!value) return null;
  const time = Date.parse(value);
  return Number.isFinite(time) ? time : null;
}

export function dashboardAge(
  value: string | null | undefined,
  now: number
): {
  value: number;
  unit: 'second' | 'minute' | 'hour' | 'day';
} | null {
  const time = dashboardTimestamp(value);
  if (time === null) return null;
  const seconds = Math.max(0, Math.floor((now - time) / 1000));
  if (seconds < 60) return { value: seconds, unit: 'second' };
  if (seconds < 3600)
    return { value: Math.floor(seconds / 60), unit: 'minute' };
  if (seconds < 86400)
    return { value: Math.floor(seconds / 3600), unit: 'hour' };
  return { value: Math.floor(seconds / 86400), unit: 'day' };
}
