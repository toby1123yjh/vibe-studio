import type { Page } from '@playwright/test';

/** Dashboard owns real queries now; shell tests isolate only its API transport. */
export async function mockEmptyDashboard(page: Page) {
  await page.route('**/api/dashboard/query', (route) => route.fulfill({ json: {
    success: true,
    data: {
      generated_at: new Date().toISOString(), snapshot_id: 'shell-snapshot', snapshot_status: 'ready',
      page: 0, page_size: 20, total: 0, latest_total: 0,
      counts: { work_count: 0, active_session_count: 0, active_run_count: 0, waiting: 0, running: 0, failed: 0, finished: 0, current: 0, all: 0 },
      rows: [], expanded: [], has_updates: false, issues: [], projects: [], providers: [],
    },
  } }));
}
