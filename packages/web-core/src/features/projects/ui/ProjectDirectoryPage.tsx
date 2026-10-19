import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { FolderKanban, LoaderCircle, Plus, Search } from 'lucide-react';
import type { ProjectListItem } from 'shared/types';
import {
  DegradedState,
  EmptyState,
  ErrorState,
  LoadingState,
  OfflineState,
} from '@vibe/ui/components/StateSurface';
import { ProjectActionsMenu } from '@/shared/components/ProjectActionsMenu';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useAppShellProjects } from '@/shared/hooks/useAppShellProjects';
import { useUserOrganizations } from '@/shared/hooks/useUserOrganizations';
import { CreateRemoteProjectDialog } from '@/shared/dialogs/org/CreateRemoteProjectDialog';
import { useSettingsNavigation } from '@/shared/hooks/useSettingsNavigation';
import { useDeleteProject } from '@/shared/hooks/useDeleteProject';
import { useOrganizationStore } from '@/shared/stores/useOrganizationStore';
import { deriveProjectDirectoryState } from '../model/projectDirectoryState';
import './project-surfaces.css';
import { isDefaultProject } from '@/shared/lib/defaultProject';

const EMPTY_PROJECTS: readonly ProjectListItem[] = [];

function formatUpdatedAt(value: string, locale: string): string {
  return new Intl.DateTimeFormat(locale, {
    dateStyle: 'medium',
  }).format(new Date(value));
}

function ProjectCard({
  project,
  locale,
  onOpen,
  onManage,
  onDelete,
  deleting,
}: {
  project: ProjectListItem;
  locale: string;
  onOpen(): void;
  onManage(): void;
  onDelete(): void;
  deleting: boolean;
}) {
  const { t } = useTranslation('projects');

  return (
    <article className="vk-project-card">
      <button
        type="button"
        className="vk-project-card__open"
        disabled={deleting}
        onClick={onOpen}
        aria-label={t('directory.openProject', {
          name: project.name,
          defaultValue: 'Open {{name}}',
        })}
      >
        <span className="vk-project-card__cover" aria-hidden="true">
          <FolderKanban size={28} />
        </span>
        <span className="vk-project-card__copy">
          <strong title={project.name}>{project.name}</strong>
          <small>
            {isDefaultProject(project.id)
              ? t('directory.builtInProject', {
                  defaultValue: 'Built-in project',
                })
              : t('directory.updated', {
                  date: formatUpdatedAt(project.updated_at, locale),
                  defaultValue: 'Updated {{date}}',
                })}
          </small>
        </span>
      </button>
      <ProjectActionsMenu
        protectedProject={isDefaultProject(project.id)}
        projectName={project.name}
        className="vk-project-card__menu-trigger"
        disabled={deleting}
        onSettings={onManage}
        onDelete={onDelete}
      />
    </article>
  );
}

export function ProjectDirectoryPage() {
  const { t, i18n } = useTranslation('projects');
  const navigation = useAppNavigation();
  const { openSettings } = useSettingsNavigation();
  const projectsState = useAppShellProjects();
  const { deleteProject: handleDelete, pendingProjectId: deletingProjectId } =
    useDeleteProject({
      scopeKey: 'directory',
      enabled:
        Boolean(projectsState?.scopeKey) &&
        !(projectsState?.deployment === 'remote' && !projectsState.hostId),
    });
  const { data: organizationData } = useUserOrganizations();
  const selectedOrganizationId = useOrganizationStore(
    (state) => state.selectedOrgId
  );
  const [query, setQuery] = useState('');
  const [isRetrying, setIsRetrying] = useState(false);
  const retryLockRef = useRef(false);
  const nextPageLockRef = useRef(false);
  const scopeEpochRef = useRef(0);
  const paginationSentinelRef = useRef<HTMLDivElement>(null);
  const scopeKey = projectsState?.scopeKey ?? 'missing-app-shell-projects';
  const projects = projectsState?.items ?? EMPTY_PROJECTS;

  useEffect(() => {
    scopeEpochRef.current += 1;
    retryLockRef.current = false;
    nextPageLockRef.current = false;
    setIsRetrying(false);
    setQuery('');
  }, [scopeKey]);

  const retry = useCallback(async () => {
    if (!projectsState || retryLockRef.current) return;
    retryLockRef.current = true;
    const epoch = scopeEpochRef.current;
    setIsRetrying(true);
    try {
      await projectsState.retry();
    } finally {
      if (scopeEpochRef.current === epoch) {
        retryLockRef.current = false;
        setIsRetrying(false);
      }
    }
  }, [projectsState]);

  const loadNextPage = useCallback(async () => {
    if (
      !projectsState?.hasNextPage ||
      projectsState.isFetchingNextPage ||
      nextPageLockRef.current
    ) {
      return;
    }
    nextPageLockRef.current = true;
    const epoch = scopeEpochRef.current;
    try {
      await projectsState.loadNextPage();
    } finally {
      if (scopeEpochRef.current === epoch) nextPageLockRef.current = false;
    }
  }, [projectsState]);

  const filteredProjects = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    return normalized
      ? projects.filter((project) =>
          project.name.toLocaleLowerCase().includes(normalized)
        )
      : projects;
  }, [projects, query]);
  const searchIsHydrating =
    Boolean(query.trim()) &&
    Boolean(projectsState?.hasNextPage) &&
    Boolean(projectsState?.isFetchingNextPage);

  useEffect(() => {
    if (
      !query.trim() ||
      !projectsState?.hasNextPage ||
      projectsState.isFetchingNextPage ||
      projectsState.isFetchNextPageError
    ) {
      return;
    }
    void loadNextPage();
  }, [
    loadNextPage,
    projectsState?.hasNextPage,
    projectsState?.isFetchNextPageError,
    projectsState?.isFetchingNextPage,
    query,
  ]);

  useEffect(() => {
    const sentinel = paginationSentinelRef.current;
    if (
      !sentinel ||
      query.trim() ||
      !projectsState?.hasNextPage ||
      projectsState.isFetchNextPageError ||
      typeof IntersectionObserver === 'undefined'
    ) {
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          void loadNextPage();
        }
      },
      { rootMargin: '240px 0px' }
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [
    loadNextPage,
    projectsState?.hasNextPage,
    projectsState?.isFetchNextPageError,
    query,
  ]);
  const organizationId =
    organizationData?.organizations.find(
      (organization) => organization.id === selectedOrganizationId
    )?.id ??
    organizationData?.organizations[0]?.id ??
    null;

  const createProject = async () => {
    if (!organizationId) return;
    const result = await CreateRemoteProjectDialog.show({ organizationId });
    if (result.action === 'created' && result.project) {
      navigation.goToProject(result.project.id, {
        hostId: result.hostId ?? null,
      });
      // Discovery refresh must not hold the user on the directory after creation.
      // Its source owns the error/retry state independently of navigation.
      void projectsState?.retry().catch(() => undefined);
    }
  };

  const isRemoteOffline =
    projectsState?.deployment === 'remote' && !projectsState.hostId;
  const retryDegraded = projectsState?.isFetchNextPageError
    ? loadNextPage
    : retry;
  const directoryState = deriveProjectDirectoryState({
    hasSource: Boolean(projectsState),
    isRemoteOffline,
    isLoading: Boolean(projectsState?.isLoading),
    isError: Boolean(projectsState?.isError),
    isFetchNextPageError: Boolean(projectsState?.isFetchNextPageError),
    itemCount: projects.length,
    visibleItemCount: filteredProjects.length,
    isSearchHydrating: searchIsHydrating,
  });

  return (
    <section className="vk-project-directory" aria-labelledby="projects-title">
      <header className="vk-project-directory__header">
        <div>
          <p>{t('directory.eyebrow', { defaultValue: 'Workspace' })}</p>
          <h1 id="projects-title">
            {t('directory.title', { defaultValue: 'Projects' })}
          </h1>
        </div>
        <button
          type="button"
          className="vk-primary-action"
          onClick={() => void createProject()}
          disabled={!organizationId || isRemoteOffline}
          title={
            organizationId
              ? undefined
              : t('directory.noOrganization', {
                  defaultValue: 'No organization is available',
                })
          }
        >
          <Plus aria-hidden="true" size={16} />
          <span>
            {t('directory.newProject', { defaultValue: 'New project' })}
          </span>
        </button>
      </header>

      <label className="vk-project-search">
        <Search aria-hidden="true" size={16} />
        <span className="vk-visually-hidden">
          {t('directory.searchLabel', { defaultValue: 'Search projects' })}
        </span>
        <input
          type="search"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={t('directory.searchPlaceholder', {
            defaultValue: 'Search projects',
          })}
        />
      </label>

      {directoryState === 'unavailable' ? (
        <ErrorState
          className="vk-project-directory__state"
          title={t('directory.unavailableTitle', {
            defaultValue: 'Projects are unavailable',
          })}
          description={t('directory.unavailableDescription', {
            defaultValue:
              'Project discovery is not connected to the application shell.',
          })}
        />
      ) : directoryState === 'offline' ? (
        <OfflineState
          className="vk-project-directory__state"
          title={t('directory.offlineTitle', {
            defaultValue: 'Select an online host',
          })}
          description={t('directory.offlineDescription', {
            defaultValue:
              'Projects are stored on a host. Connect to a host to browse them.',
          })}
        />
      ) : directoryState === 'loading' ? (
        <LoadingState
          className="vk-project-directory__state"
          title={t('directory.loadingTitle', {
            defaultValue: 'Loading projects',
          })}
        />
      ) : directoryState === 'error' ? (
        <ErrorState
          compact
          className="vk-project-directory__state vk-project-directory__state--inline"
          title={t('directory.errorTitle', {
            defaultValue: 'Projects could not be loaded',
          })}
          description={t('directory.errorDescription', {
            defaultValue:
              'Check the connection and try the same request again.',
          })}
          action={
            <button
              type="button"
              className="vk-state-action"
              onClick={() => void retry()}
              disabled={isRetrying}
            >
              {isRetrying
                ? t('directory.retrying', { defaultValue: 'Retrying…' })
                : t('directory.retry', { defaultValue: 'Retry' })}
            </button>
          }
        />
      ) : directoryState === 'empty' ? (
        <EmptyState
          className="vk-project-directory__state"
          title={
            query
              ? t('directory.noMatchesTitle', {
                  defaultValue: 'No matching projects',
                })
              : t('directory.emptyTitle', { defaultValue: 'No projects yet' })
          }
          description={
            query
              ? t('directory.noMatchesDescription', {
                  defaultValue: 'Try a different project name.',
                })
              : t('directory.emptyDescription', {
                  defaultValue: 'Create a project to organise tasks and tasks.',
                })
          }
        />
      ) : (
        <>
          {directoryState === 'degraded' ? (
            <DegradedState
              compact
              className="vk-project-directory__degraded"
              title={t('directory.degradedTitle', {
                defaultValue: 'Projects may be out of date',
              })}
              description={t('directory.degradedDescription', {
                defaultValue:
                  'Loaded projects remain available while the failed request is retried.',
              })}
              action={
                <button
                  type="button"
                  className="vk-state-action"
                  onClick={() => void retryDegraded()}
                  disabled={
                    isRetrying || Boolean(projectsState?.isFetchingNextPage)
                  }
                >
                  {isRetrying || projectsState?.isFetchingNextPage
                    ? t('directory.retrying', { defaultValue: 'Retrying…' })
                    : t('directory.retry', { defaultValue: 'Retry' })}
                </button>
              }
            />
          ) : null}
          {filteredProjects.length === 0 && !searchIsHydrating ? (
            <EmptyState
              className="vk-project-directory__state"
              title={t('directory.noMatchesTitle', {
                defaultValue: 'No matching projects',
              })}
              description={t('directory.noMatchesDescription', {
                defaultValue: 'Try a different project name.',
              })}
            />
          ) : (
            <div className="vk-project-grid">
              {filteredProjects.map((project) => (
                <ProjectCard
                  key={`${scopeKey}:${project.id}`}
                  project={project}
                  locale={i18n.resolvedLanguage ?? i18n.language}
                  onOpen={() => navigation.goToProject(project.id)}
                  onManage={() =>
                    openSettings('projects', {
                      projectId: project.id,
                      hostId: projectsState?.hostId,
                    })
                  }
                  onDelete={() => void handleDelete(project)}
                  deleting={deletingProjectId !== null}
                />
              ))}
            </div>
          )}
          {searchIsHydrating ? (
            <div className="vk-project-directory__progress" role="status">
              <LoaderCircle className="vk-spin" aria-hidden="true" size={16} />
              {t('directory.searchingRemaining', {
                defaultValue: 'Searching remaining projects…',
              })}
            </div>
          ) : null}
        </>
      )}

      {projectsState?.hasNextPage &&
      !query.trim() &&
      directoryState !== 'degraded' ? (
        <div
          ref={paginationSentinelRef}
          className="vk-project-directory__sentinel"
          role="status"
          aria-live="polite"
          aria-busy={projectsState.isFetchingNextPage}
        >
          {projectsState.isFetchingNextPage ? (
            <>
              <LoaderCircle className="vk-spin" aria-hidden="true" size={16} />
              {t('directory.loadingMore', {
                defaultValue: 'Loading more projects…',
              })}
            </>
          ) : (
            <span className="vk-visually-hidden">
              {t('directory.moreAvailable', {
                defaultValue: 'More projects load automatically on scroll.',
              })}
            </span>
          )}
        </div>
      ) : null}
    </section>
  );
}
