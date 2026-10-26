import {
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { useInfiniteQuery } from '@tanstack/react-query';
import {
  useLocation,
  useNavigate,
  useRouterState,
} from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import type {
  ProjectCursor,
  ProjectListItem,
  SessionCursor,
  SessionListItem,
} from 'shared/types';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useCurrentAppDestination } from '@/shared/hooks/useCurrentAppDestination';
import { useRenderedPathname } from '@/shared/hooks/useRenderedPathname';
import { executionDataApi } from '@/shared/lib/executionDataApi';
import { useDeleteExecutionSession } from '@/shared/hooks/useDeleteExecutionSession';
import { getProjectDestination } from '@/shared/lib/routes/appNavigation';
import { useIsMobile } from '@/shared/hooks/useIsMobile';
import { useVisualViewportHeightVar } from '@/shared/hooks/useVisualViewportHeightVar';
import { useUiPreferencesStore } from '@/shared/stores/useUiPreferencesStore';
import { AppShellRecentSessionsProvider } from '@/shared/hooks/useAppShellRecentSessions';
import { AppShellProjectsProvider } from '@/shared/hooks/useAppShellProjects';
import { WorkspaceContext } from '@/shared/hooks/useWorkspaceContext';
import { DashboardScopeProvider } from '@/features/session-dashboard/model/DashboardScope';
import {
  appShellDiscoveryQueryKey,
  deriveActiveShellModule,
  derivePageCanvasMode,
  mergeStableCursorItems,
  sessionRoute,
  type AppShellCapabilityAdapter,
} from '../model/appShell';
import {
  deriveSearchSourceState,
  isSearchRouteAvailable,
} from '../model/search';
import { GlobalSearchPalette } from '../ui/GlobalSearchPalette';
import { PageCanvas } from '../ui/PageCanvas';
import { ProductSidebar } from '../ui/ProductSidebar';
import '../ui/app-shell.css';

interface AppShellContainerProps {
  adapter: AppShellCapabilityAdapter;
  banner?: ReactNode;
  children: ReactNode;
}

export function AppShellContainer({
  adapter,
  banner,
  children,
}: AppShellContainerProps) {
  const { t } = useTranslation('common');
  const location = useLocation();
  const navigate = useNavigate();
  const navigateDashboard = useCallback(
    (href: string) => {
      void navigate({ href });
    },
    [navigate]
  );
  const renderedPathname = useRenderedPathname();
  const routeSessionId = useRouterState({
    select: (state) => {
      const search = state.matches.at(-1)?.search as
        | Record<string, unknown>
        | undefined;
      return typeof search?.session_id === 'string' && search.session_id
        ? search.session_id
        : null;
    },
  });
  const workspaceContext = useContext(WorkspaceContext);
  const appNavigation = useAppNavigation();
  const currentDestination = useCurrentAppDestination();
  const isMobile = useIsMobile();
  useVisualViewportHeightVar(isMobile);
  const mobileFontScale = useUiPreferencesStore(
    (state) => state.mobileFontScale
  );
  const [searchOpen, setSearchOpen] = useState(false);
  const [objectDrawerOpen, setObjectDrawerOpen] = useState(false);
  const searchTriggerRef = useRef<HTMLElement | null>(null);
  const mainContentRef = useRef<HTMLElement>(null);
  const previousLocationRef = useRef(location.href);

  useEffect(() => {
    if (!isMobile) {
      document.documentElement.style.removeProperty('--mobile-font-scale');
      return;
    }
    const scale = { default: '1', small: '0.9', smaller: '0.8' } as const;
    document.documentElement.style.setProperty(
      '--mobile-font-scale',
      scale[mobileFontScale]
    );
    return () => {
      document.documentElement.style.removeProperty('--mobile-font-scale');
    };
  }, [isMobile, mobileFontScale]);

  const projectsQuery = useInfiniteQuery({
    meta: { hostId: adapter.discoveryHostId },
    queryKey: appShellDiscoveryQueryKey(adapter.discoveryScopeKey, 'projects'),
    queryFn: ({ pageParam, signal }) =>
      executionDataApi.listProjects({
        cursor: pageParam,
        limit: 50,
        hostId: adapter.discoveryHostId,
        signal,
      }),
    initialPageParam: null as ProjectCursor | null,
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    staleTime: 30_000,
    // The local web shell can mount before the backend has finished starting.
    // Keep discovery recoverable without requiring the user to refresh the UI.
    retry: 5,
    retryDelay: (attemptIndex) => Math.min(1000 * 2 ** attemptIndex, 8000),
    refetchInterval: (query) => (query.state.status === 'error' ? 5000 : false),
  });

  const sessionsQuery = useInfiniteQuery({
    meta: { hostId: adapter.discoveryHostId },
    queryKey: appShellDiscoveryQueryKey(adapter.discoveryScopeKey, 'sessions'),
    queryFn: ({ pageParam, signal }) =>
      executionDataApi.listRecentSessions({
        cursor: pageParam,
        limit: 50,
        hostId: adapter.discoveryHostId,
        signal,
      }),
    initialPageParam: null as SessionCursor | null,
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    staleTime: 15_000,
    retry: 5,
    retryDelay: (attemptIndex) => Math.min(1000 * 2 ** attemptIndex, 8000),
    refetchInterval: (query) => (query.state.status === 'error' ? 5000 : false),
  });

  const projects = useMemo(
    () =>
      (projectsQuery.data?.pages ?? []).reduce(
        (items, page) => mergeStableCursorItems(items, page.projects),
        [] as ProjectListItem[]
      ),
    [projectsQuery.data?.pages]
  );
  const sessions = useMemo(
    () =>
      (sessionsQuery.data?.pages ?? []).reduce(
        (items, page) => mergeStableCursorItems(items, page.sessions),
        [] as SessionListItem[]
      ),
    [sessionsQuery.data?.pages]
  );
  const refetchProjects = projectsQuery.refetch;
  const fetchNextProjectPage = projectsQuery.fetchNextPage;
  const refetchSessions = sessionsQuery.refetch;

  // A module owns its nested routes, even when their data belongs to a project.
  const activeModule = deriveActiveShellModule(renderedPathname);
  const activeProjectId = activeModule
    ? null
    : (getProjectDestination(currentDestination)?.projectId ?? null);
  const activeWorkspaceId =
    currentDestination && 'workspaceId' in currentDestination
      ? currentDestination.workspaceId
      : null;
  const selectedSessionId =
    workspaceContext?.workspaceId === activeWorkspaceId
      ? workspaceContext.isNewSessionMode
        ? null
        : (workspaceContext.selectedSessionId ?? routeSessionId)
      : routeSessionId;
  // Sidebar selection belongs to the rendered page, not a retained
  // conversation or a session shown inside a project/workflow/dashboard panel.
  const activeSessionId =
    !activeModule &&
    (currentDestination?.kind === 'workspace' ||
      currentDestination?.kind === 'workspace-vscode')
      ? selectedSessionId
      : null;
  const openSearch = useCallback(() => {
    searchTriggerRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setObjectDrawerOpen(false);
    setSearchOpen(true);
  }, []);

  const closeSearch = useCallback((options?: { restoreFocus?: boolean }) => {
    setSearchOpen(false);
    if (options?.restoreFocus === false) return;
    requestAnimationFrame(() => {
      if (searchTriggerRef.current?.isConnected) {
        searchTriggerRef.current.focus({ preventScroll: true });
      }
    });
  }, []);

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        event.stopPropagation();
        if (!searchOpen) openSearch();
      }
    };
    window.addEventListener('keydown', handleShortcut, { capture: true });
    return () =>
      window.removeEventListener('keydown', handleShortcut, { capture: true });
  }, [openSearch, searchOpen]);

  useEffect(() => setObjectDrawerOpen(false), [location.pathname]);

  useEffect(() => {
    if (previousLocationRef.current === location.href) return;
    previousLocationRef.current = location.href;
    const frame = requestAnimationFrame(() => {
      mainContentRef.current?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [location.href]);

  const projectState = useMemo(
    () => ({
      scopeKey: adapter.discoveryScopeKey,
      deployment: adapter.deployment,
      hostId: adapter.discoveryHostId,
      items: projects,
      isLoading: projectsQuery.isLoading,
      isError: projectsQuery.isError,
      isFetching: projectsQuery.isFetching,
      isFetchNextPageError: projectsQuery.isFetchNextPageError,
      hasNextPage: projectsQuery.hasNextPage,
      isFetchingNextPage: projectsQuery.isFetchingNextPage,
      retry: async () => {
        await refetchProjects();
      },
      loadNextPage: async () => {
        await fetchNextProjectPage();
      },
    }),
    [
      adapter.deployment,
      adapter.discoveryHostId,
      adapter.discoveryScopeKey,
      projects,
      fetchNextProjectPage,
      projectsQuery.hasNextPage,
      projectsQuery.isError,
      projectsQuery.isFetchNextPageError,
      projectsQuery.isFetching,
      projectsQuery.isFetchingNextPage,
      projectsQuery.isLoading,
      refetchProjects,
    ]
  );
  const sessionState = {
    items: sessions,
    isLoading: sessionsQuery.isLoading,
    isError: sessionsQuery.isError,
    hasNextPage: sessionsQuery.hasNextPage,
    isFetchingNextPage: sessionsQuery.isFetchingNextPage,
    retry: () => void sessionsQuery.refetch(),
    loadNextPage: () => void sessionsQuery.fetchNextPage(),
  };
  const searchSources = useMemo(
    () => [
      {
        id: 'projects' as const,
        state: deriveSearchSourceState(projectsQuery.isError, projects.length),
        retry: (): void => {
          void refetchProjects();
        },
      },
      {
        id: 'sessions' as const,
        state: deriveSearchSourceState(sessionsQuery.isError, sessions.length),
        retry: (): void => {
          void refetchSessions();
        },
      },
    ],
    [
      projects.length,
      projectsQuery.isError,
      refetchProjects,
      refetchSessions,
      sessions.length,
      sessionsQuery.isError,
    ]
  );
  const navigateFromSearch = useCallback(
    (route: string) => {
      if (!isSearchRouteAvailable(route, adapter.moduleCapabilities))
        return false;
      adapter.navigateToRoute(route);
      return true;
    },
    [adapter]
  );

  const { deleteSession, pendingSessionId: deletingSessionId } =
    useDeleteExecutionSession({
      hostId: adapter.discoveryHostId,
      discoveryScopeKey: adapter.discoveryScopeKey,
      scopeKey: JSON.stringify([
        adapter.discoveryScopeKey,
        location.href,
        selectedSessionId,
      ]),
      onDeleted: (target) => {
        if (selectedSessionId !== target.sessionId) return;
        const nextSession = sessions.find(
          (item) => item.id !== target.sessionId
        );
        adapter.navigateToRoute(
          nextSession
            ? sessionRoute(nextSession)
            : `/workspaces/${encodeURIComponent(target.workspaceId)}`
        );
      },
    });

  return (
    <AppShellProjectsProvider value={projectState}>
      <AppShellRecentSessionsProvider sessions={sessions}>
        <div
          className="vk-app-shell"
          style={isMobile ? { height: 'var(--app-vh, 100dvh)' } : undefined}
          data-deployment={adapter.deployment}
        >
          <a className="vk-skip-link" href="#main-content">
            {t('appShell.skipToContent', { defaultValue: 'Skip to content' })}
          </a>
          <div className="vk-app-shell__layout" data-search-open={searchOpen}>
            <ProductSidebar
              adapter={adapter}
              activeModule={activeModule}
              activeProjectId={activeProjectId}
              activeSessionId={activeSessionId}
              projects={projectState}
              sessions={sessionState}
              newSession={
                appNavigation.agentExecutionUnavailableReason
                  ? {
                      availability: 'unavailable',
                      reason: appNavigation.agentExecutionUnavailableReason,
                    }
                  : {
                      availability: 'available',
                      navigate: () => appNavigation.goToWorkspacesCreate(),
                    }
              }
              objectDrawerOpen={objectDrawerOpen}
              onObjectDrawerOpenChange={setObjectDrawerOpen}
              onSearch={openSearch}
              onProject={(projectId) => appNavigation.goToProject(projectId)}
              onSession={(session) =>
                adapter.navigateToRoute(sessionRoute(session))
              }
              onDeleteSession={(session) =>
                void deleteSession({
                  sessionId: session.id,
                  workspaceId: session.workspace_id,
                  projectId: session.project_id,
                  title: session.title,
                })
              }
              deletingSessionId={deletingSessionId}
            />
            <div className="vk-page-stack">
              {banner}
              <PageCanvas
                ref={mainContentRef}
                mode={derivePageCanvasMode(renderedPathname)}
              >
                <DashboardScopeProvider
                  value={{
                    hostId: adapter.discoveryHostId,
                    scopeKey: adapter.discoveryScopeKey,
                    deployment: adapter.deployment,
                    navigateToRoute: navigateDashboard,
                  }}
                >
                  {children}
                </DashboardScopeProvider>
              </PageCanvas>
            </div>
          </div>
          <GlobalSearchPalette
            key={adapter.discoveryScopeKey}
            open={searchOpen}
            scopeKey={adapter.discoveryScopeKey}
            projects={projects}
            sessions={sessions}
            sources={searchSources}
            moduleCapabilities={adapter.moduleCapabilities}
            onClose={closeSearch}
            onNavigate={navigateFromSearch}
          />
        </div>
      </AppShellRecentSessionsProvider>
    </AppShellProjectsProvider>
  );
}
