import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useWorkspaces } from '@/shared/hooks/useWorkspaces';
import { workspaceSummaryKeys } from '@/shared/hooks/workspaceSummaryKeys';
import type { WorkspaceSummary, WorkspaceWithStatus } from 'shared/types';
import { FixtureHostProvider } from './host';
import { sockets, summaryRequests, summaryResponses } from './transport';

const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false } },
});
let current: ReturnType<typeof useWorkspaces>;
let baseline: ReturnType<typeof useWorkspaces>;
let listChanges = 0;
let lastLists: unknown[] = [];

function summary(archived: boolean, filesChanged = 2): WorkspaceSummary {
  return {
    workspace_id: archived ? 'workspace-archived' : 'workspace-active',
    latest_session_id: null,
    has_pending_approval: false,
    files_changed: filesChanged,
    lines_added: 4,
    lines_removed: 1,
    latest_process_status: null,
    has_running_dev_server: false,
    has_unseen_turns: false,
    pr_status: null,
    pr_number: null,
    pr_url: null,
  };
}

for (const hostId of ['host-a', 'host-b']) {
  for (const archived of [false, true]) {
    summaryResponses.set(`${hostId}:${archived}`, [summary(archived)]);
  }
}

const fixture = {
  requests: summaryRequests,
  initialize(hostId = 'host-a') {
    for (const socket of sockets.filter(
      (item) => item.transport?.hostId === hostId
    )) {
      const archived = socket.endpoint.endsWith('archived=true');
      const id = archived ? 'workspace-archived' : 'workspace-active';
      const workspace: WorkspaceWithStatus = {
        id,
        name: `${hostId}:${id}`,
        branch: 'main',
        container_ref: null,
        workspace_kind: 'direct_folder',
        container_ownership: 'external',
        setup_completed_at: null,
        created_at: '2026-10-27T00:00:00Z',
        updated_at: '2026-10-27T00:00:00Z',
        archived,
        pinned: false,
        worktree_deleted: false,
        is_running: false,
        is_errored: false,
      };
      socket.sendEvent({
        JsonPatch: [
          { op: 'replace', path: '/workspaces', value: { [id]: workspace } },
        ],
      });
      socket.sendEvent({ Ready: true });
    }
  },
  capture() {
    baseline = current;
    return listChanges;
  },
  compare() {
    return {
      activeList: current.workspaces === baseline.workspaces,
      archivedList: current.archivedWorkspaces === baseline.archivedWorkspaces,
      activeRow: current.workspaces[0] === baseline.workspaces[0],
      archivedRow:
        current.archivedWorkspaces[0] === baseline.archivedWorkspaces[0],
      listChanges,
    };
  },
  change(hostId: string, archived: boolean, filesChanged: number) {
    summaryResponses.set(`${hostId}:${archived}`, [
      summary(archived, filesChanged),
    ]);
  },
  async refetch() {
    await queryClient.refetchQueries({ queryKey: workspaceSummaryKeys.all });
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
    );
  },
};
Object.assign(window, { workspaceSummaryFixture: fixture });

function Workspaces() {
  current = useWorkspaces();
  if (
    lastLists[0] !== current.workspaces ||
    lastLists[1] !== current.archivedWorkspaces
  ) {
    ++listChanges;
    lastLists = [current.workspaces, current.archivedWorkspaces];
  }
  return (
    <>
      <output data-testid="state">{current.state}</output>
      <output data-testid="name">{current.workspaces[0]?.name}</output>
      <output data-testid="files">{current.workspaces[0]?.filesChanged}</output>
    </>
  );
}

function App() {
  const [hostId, setHostId] = useState('host-a');
  return (
    <FixtureHostProvider value={hostId}>
      <button onClick={() => setHostId('host-b')}>Switch Host</button>
      <Workspaces />
    </FixtureHostProvider>
  );
}

createRoot(document.getElementById('root')!).render(
  <QueryClientProvider client={queryClient}>
    <App />
  </QueryClientProvider>
);
