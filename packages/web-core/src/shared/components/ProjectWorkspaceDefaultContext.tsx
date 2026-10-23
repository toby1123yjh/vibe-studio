import { useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Folder } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import {
  getProjectWorkspaceDefaultOrThrow,
  projectWorkspaceDefaultQueryKey,
} from '@/shared/hooks/useProjectRepoDefaults';
import { repoApi } from '@/shared/lib/api';
import { buildWorkspaceContext } from '@/shared/lib/workspaceContext';
import { cn } from '@/shared/lib/utils';
import { Tooltip } from '@vibe/ui/components/Tooltip';

interface ProjectWorkspaceDefaultContextProps {
  projectId: string;
  hostId?: string | null;
  className?: string;
}

export function ProjectWorkspaceDefaultContext({
  projectId,
  hostId,
  className,
}: ProjectWorkspaceDefaultContextProps) {
  const { t } = useTranslation('common');
  const defaultContextQuery = useQuery({
    queryKey: projectWorkspaceDefaultQueryKey(projectId, hostId),
    queryFn: async () => {
      const workspaceDefault = await getProjectWorkspaceDefaultOrThrow(
        projectId,
        hostId
      );
      if (!workspaceDefault) return null;
      if (workspaceDefault.kind === 'direct_folder') {
        return { path: workspaceDefault.path, branch: undefined };
      }
      if (!workspaceDefault.repo.target_branch.trim()) {
        throw new Error('Project default branch is empty');
      }

      const repo = await repoApi.getById(workspaceDefault.repo.repo_id, hostId);
      return {
        path: repo.path,
        branch: workspaceDefault.repo.target_branch,
      };
    },
    refetchOnMount: 'always',
  });

  const parts = useMemo(
    () =>
      defaultContextQuery.data
        ? buildWorkspaceContext({
            containerRef: defaultContextQuery.data.path,
            branch: defaultContextQuery.data.branch,
          })
        : [],
    [defaultContextQuery.data]
  );
  const workspaceContextLabel = useMemo(
    () => parts.map((part) => part.label).join(' / '),
    [parts]
  );

  const isConfigured = parts.length > 0;

  return (
    <div
      className={cn(
        'flex min-w-0 items-center gap-half text-sm text-low',
        className
      )}
      aria-label={t('projectWorkspaceDefault.label', {
        defaultValue: 'Working directory',
      })}
    >
      <Folder className="size-icon-xs shrink-0" aria-hidden="true" />

      {defaultContextQuery.isLoading ? (
        <span
          className="h-3 w-48 max-w-full animate-pulse rounded-sm bg-secondary motion-reduce:animate-none"
          aria-label={t('projectWorkspaceDefault.loading', {
            defaultValue: 'Loading working directory',
          })}
        />
      ) : defaultContextQuery.isError ? (
        <span className="truncate" role="status">
          {t('projectWorkspaceDefault.loadFailed', {
            defaultValue: 'Working directory unavailable',
          })}
        </span>
      ) : isConfigured ? (
        <Tooltip
          content={workspaceContextLabel}
          side="bottom"
          className="max-w-[min(48rem,calc(100vw-2rem))] break-all font-ibm-plex-mono"
        >
          <div
            className="flex min-w-0 flex-1 items-center gap-half font-ibm-plex-mono"
            aria-label={workspaceContextLabel}
            title={workspaceContextLabel}
          >
            {parts.map((part, index) => (
              <div key={`${part.kind}-${part.label}`} className="contents">
                {index > 0 ? <span className="shrink-0">/</span> : null}
                <span
                  className={cn(
                    'min-w-0 truncate',
                    part.kind === 'path' ? 'flex-1' : 'max-w-[120px]'
                  )}
                >
                  {part.label}
                </span>
              </div>
            ))}
          </div>
        </Tooltip>
      ) : (
        <span className="truncate">
          {t('projectWorkspaceDefault.empty', {
            defaultValue: 'No working directory configured',
          })}
        </span>
      )}
    </div>
  );
}
