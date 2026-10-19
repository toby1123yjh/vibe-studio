import { expect, test } from '@playwright/test';
import { dashboardActivityKey, dashboardAge, dashboardConversationKey, dashboardQueryKey, dashboardRowKey, dashboardTargetHref, DEFAULT_DASHBOARD_FILTERS, reconcileDashboardExpanded } from '../../../packages/web-core/src/features/session-dashboard/model/dashboard';
import { dashboardResponse, sessionTarget } from '../support/dashboard-api';

test('query, row and conversation identities isolate scope, pages and exact sessions/runs', () => {
  const row = dashboardResponse().rows[0];
  expect(dashboardRowKey('a', row.work)).not.toBe(dashboardRowKey('b', row.work));
  const key = dashboardQueryKey('a:user1', DEFAULT_DASHBOARD_FILTERS, 'snap', 0, []);
  expect(key).not.toEqual(dashboardQueryKey('a:user2', DEFAULT_DASHBOARD_FILTERS, 'snap', 0, []));
  expect(key).not.toEqual(dashboardQueryKey('a:user1', DEFAULT_DASHBOARD_FILTERS, 'snap', 1, []));
  expect(dashboardConversationKey('a', sessionTarget())).not.toBe(dashboardConversationKey('a', { ...sessionTarget(), agent_run_id: 'retry' }));
});

test('precise navigation preserves Host, Session and workflow/Arena identities', () => {
  expect(dashboardTargetHref('host a', sessionTarget())).toBe('/hosts/host%20a/workspaces/visual-workspace?session_id=analysis-session');
  const row = dashboardResponse().rows[1];
  expect(dashboardTargetHref('a', row.open_target, row)).toBe('/hosts/a/projects/project-1/workflow-runs/visual-run');
  expect(dashboardTargetHref(null, dashboardResponse().rows[3].open_target, row)).toBe('/projects/project-1/tasks/task-1/arena/arena-1');
  expect(dashboardTargetHref(null, { kind: 'arena', arena_group_id: 'group', project_id: null })).toBeNull();
});

test('retrying a workflow invalidates only its child page while keeping the parent identity', () => {
  const row = dashboardResponse().rows[1];
  const expanded = [{ work: row.work, workflow_run_id: 'old-run', snapshot_id: 'old-page', page: 2 }];
  expect(reconcileDashboardExpanded(expanded, [row])).toEqual([{ ...expanded[0], workflow_run_id: 'visual-run', snapshot_id: null, page: 0 }]);
  expect(reconcileDashboardExpanded(expanded, [{ ...row, invalidated: true }])).toEqual([]);
});

test('tool completion has a distinct label and malformed time stays unavailable', () => {
  expect(dashboardActivityKey('tool', 'running')).toBe('sessionDashboard.toolStatuses.running');
  expect(dashboardActivityKey('tool', 'succeeded')).toBe('sessionDashboard.toolStatuses.succeeded');
  expect(dashboardActivityKey('tool', undefined)).toBe('sessionDashboard.toolStatuses.unknown');
  expect(dashboardAge('invalid', Date.now())).toBeNull();
  expect(dashboardAge('2026-10-19T00:00:00Z', Date.parse('2026-10-19T00:01:10Z'))).toEqual({ value: 1, unit: 'minute' });
});
