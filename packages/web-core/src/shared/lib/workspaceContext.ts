import type { WorkspaceKind } from 'shared/types';

export type WorkspaceContextPartKind = 'path' | 'branch' | 'mode';

export interface WorkspaceContextPart {
  kind: WorkspaceContextPartKind;
  label: string;
}

export interface BuildWorkspaceContextOptions {
  containerRef?: string | null;
  workingDir?: string | null;
  fallbackRepoName?: string | null;
  branch?: string | null;
  workspaceKind?: WorkspaceKind | null;
  worktreeLabel?: string;
}

export function joinWorkspacePath(
  containerRef: string,
  workingDir?: string | null
) {
  const relativePath = workingDir?.trim().replace(/^[\\/]+/, '');
  if (!relativePath) return containerRef;

  const separator = containerRef.includes('\\') ? '\\' : '/';
  const basePath = containerRef.replace(/[\\/]+$/, '');
  if (!basePath) return `${separator}${relativePath}`;

  return `${basePath}${separator}${relativePath}`;
}

export function resolveWorkspaceWorkingDirectory({
  containerRef,
  workingDir,
  fallbackRepoName,
}: Pick<
  BuildWorkspaceContextOptions,
  'containerRef' | 'workingDir' | 'fallbackRepoName'
>): string | undefined {
  const normalizedContainerRef = containerRef?.trim();
  if (!normalizedContainerRef) return undefined;

  return joinWorkspacePath(
    normalizedContainerRef,
    workingDir?.trim() || fallbackRepoName?.trim()
  );
}

export function formatWorkspacePathForDisplay(path: string): string {
  if (/^\\\\\?\\UNC\\/i.test(path)) {
    return `\\\\${path.slice(8)}`;
  }
  if (/^\\\\\?\\[a-z]:\\/i.test(path)) {
    return path.slice(4);
  }
  return path;
}

export function buildWorkspaceContext({
  containerRef,
  workingDir,
  fallbackRepoName,
  branch,
  workspaceKind,
  worktreeLabel = 'Worktree',
}: BuildWorkspaceContextOptions): WorkspaceContextPart[] {
  const parts: WorkspaceContextPart[] = [];
  const workspacePath = resolveWorkspaceWorkingDirectory({
    containerRef,
    workingDir,
    fallbackRepoName,
  });

  if (workspacePath) {
    parts.push({
      kind: 'path',
      label: formatWorkspacePathForDisplay(workspacePath),
    });
  }

  const normalizedBranch = branch?.trim();
  if (normalizedBranch && normalizedBranch !== 'direct-folder') {
    parts.push({ kind: 'branch', label: normalizedBranch });
  }

  if (workspaceKind === 'worktree') {
    parts.push({ kind: 'mode', label: worktreeLabel });
  }

  return parts;
}
