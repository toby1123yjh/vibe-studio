import type { ProjectListItem, SessionListItem } from 'shared/types';

export const SHELL_MODULE_ORDER = [
  'dashboard',
  'search',
  'projects',
  'workflows',
  'agents',
] as const;

export type ShellModule = (typeof SHELL_MODULE_ORDER)[number];
export type NavigableShellModule = Exclude<ShellModule, 'search'>;

export type AppShellModuleCapability =
  | {
      availability: 'available';
      navigate(): void;
    }
  | {
      availability: 'unavailable';
      reason: string;
    };

export type PageCanvasMode = 'contained' | 'full-bleed';

export interface AppShellUpdateNotice {
  phase: 'available' | 'restart-ready';
  version: string;
  open(): void;
}

export interface AppShellCapabilityAdapter {
  deployment: 'local' | 'remote';
  /** Host identity used for discovery requests; null means the local app host. */
  discoveryHostId: string | null;
  discoveryScopeKey: string;
  environmentLabel: string;
  versionLabel?: string | null;
  updateNotice?: AppShellUpdateNotice | null;
  userLabel?: string | null;
  moduleCapabilities: Readonly<
    Record<NavigableShellModule, AppShellModuleCapability>
  >;
  navigateToRoute(route: string): void;
  openSettings(): void;
  openUser?(): void;
}

export type AppShellDiscoverySource = 'projects' | 'sessions';

export function createAppShellDiscoveryScopeKey(options: {
  deployment: AppShellCapabilityAdapter['deployment'];
  hostId: string | null;
  userId: string | null;
}): string {
  return JSON.stringify([
    options.deployment,
    options.hostId ?? 'no-host',
    options.userId ?? 'anonymous',
  ]);
}

export function appShellDiscoveryQueryKey(
  scopeKey: string,
  source: AppShellDiscoverySource
) {
  return ['app-shell', 'discovery', scopeKey, source] as const;
}

export type SidebarSectionViewState =
  | 'loading'
  | 'empty'
  | 'error'
  | 'degraded'
  | 'ready';

export function deriveSidebarSectionViewState(options: {
  itemCount: number;
  isLoading: boolean;
  isError: boolean;
}): SidebarSectionViewState {
  if (options.itemCount > 0) {
    return options.isError ? 'degraded' : 'ready';
  }
  if (options.isLoading) return 'loading';
  if (options.isError) return 'error';
  return 'empty';
}

export function activateShellModuleCapability(
  capability: AppShellModuleCapability
): boolean {
  if (capability.availability === 'unavailable') return false;
  capability.navigate();
  return true;
}

export interface StableTimestampedItem {
  id: string;
  updated_at: string;
}

export function compareStableUpdatedAt(
  left: StableTimestampedItem,
  right: StableTimestampedItem
): number {
  const byTime = right.updated_at.localeCompare(left.updated_at);
  return byTime === 0 ? left.id.localeCompare(right.id) : byTime;
}

export function mergeStableCursorItems<T extends StableTimestampedItem>(
  current: readonly T[],
  incoming: readonly T[]
): T[] {
  const byId = new Map<string, T>();
  for (const item of current) byId.set(item.id, item);
  for (const item of incoming) byId.set(item.id, item);
  return [...byId.values()].sort(compareStableUpdatedAt);
}

export function deriveActiveShellModule(pathname: string): ShellModule | null {
  const normalizedPath = normalizePathname(pathname);
  if (normalizedPath === '/' || normalizedPath.startsWith('/dashboard')) {
    return 'dashboard';
  }
  if (
    normalizedPath === '/workflows' ||
    normalizedPath.includes('/workflows') ||
    normalizedPath.includes('/workflow-runs/')
  ) {
    return 'workflows';
  }
  if (normalizedPath === '/agents' || normalizedPath.startsWith('/agents/')) {
    return 'agents';
  }
  // Individual projects own their selection in the project list.
  if (normalizedPath === '/projects') {
    return 'projects';
  }
  return null;
}

export function derivePageCanvasMode(pathname: string): PageCanvasMode {
  const normalizedPath = normalizePathname(pathname);
  if (
    normalizedPath === '/projects' ||
    normalizedPath === '/workflows' ||
    normalizedPath === '/settings' ||
    normalizedPath === '/agents' ||
    normalizedPath.startsWith('/agents/')
  ) {
    return 'contained';
  }
  return 'full-bleed';
}

function normalizePathname(pathname: string): string {
  if (pathname === '/') return pathname;
  return pathname.replace(/\/+$/, '');
}

export function projectRoute(project: ProjectListItem): string {
  return `/projects/${encodeURIComponent(project.id)}`;
}

export function sessionRoute(session: SessionListItem): string {
  const workspaceId = encodeURIComponent(session.workspace_id);
  const sessionId = encodeURIComponent(session.id);
  return `/workspaces/${workspaceId}?session_id=${sessionId}`;
}
