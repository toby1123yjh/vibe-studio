import { BaseCodingAgent } from 'shared/types';

export type ExecutorDiscoveryContext = {
  workspaceId?: string;
  sessionId?: string;
  repoId?: string;
  variant?: string;
  refresh?: boolean;
};

// Every catalog belongs to the selected profile's native command/environment.
export function resolveModelDiscoveryVariant(
  agent: BaseCodingAgent | null | undefined,
  selectedPreset: string | null
): string | undefined {
  return agent && selectedPreset?.trim() ? selectedPreset : undefined;
}

export function buildExecutorDiscoveryStreamUrl(
  agent: BaseCodingAgent,
  opts?: ExecutorDiscoveryContext
): string {
  const params = new URLSearchParams();
  params.set('executor', agent);
  if (opts?.workspaceId) params.set('workspace_id', opts.workspaceId);
  if (opts?.sessionId) params.set('session_id', opts.sessionId);
  if (opts?.repoId) params.set('repo_id', opts.repoId);
  if (opts?.variant) params.set('variant', opts.variant);
  if (opts?.refresh) params.set('refresh', 'true');
  return `/api/agents/discovered-options/ws?${params.toString()}`;
}
