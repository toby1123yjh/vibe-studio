import * as React from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ProjectListItem, SessionListItem } from 'shared/types';
import { deriveActiveShellModule } from '../../../../packages/web-core/src/features/app-shell/model/appShell';
import type { AppShellCapabilityAdapter } from '../../../../packages/web-core/src/features/app-shell/model/appShell';
import { deriveSearchSourceState } from '../../../../packages/web-core/src/features/app-shell/model/search';
import { DashboardPage } from '../../../../packages/web-core/src/features/app-shell/ui/DashboardPage';
import { GlobalSearchPalette } from '../../../../packages/web-core/src/features/app-shell/ui/GlobalSearchPalette';
import { PageCanvas } from '../../../../packages/web-core/src/features/app-shell/ui/PageCanvas';
import { ProductSidebar } from '../../../../packages/web-core/src/features/app-shell/ui/ProductSidebar';
import type { SidebarSectionState } from '../../../../packages/web-core/src/features/app-shell/ui/ProductSidebar';
import i18n from '../../../../packages/web-core/src/i18n/config';
import '../../../../packages/ui/src/styles/tokens.css';
import '../../../../packages/web-core/src/features/app-shell/ui/app-shell.css';
import './style.css';

const requestedVolume =
  typeof window === 'undefined'
    ? null
    : new URLSearchParams(window.location.search).get('volume');
const PROJECT_COUNT = requestedVolume === '200' ? 200 : 24;
const PROJECTS: ProjectListItem[] = Array.from(
  { length: PROJECT_COUNT },
  (_, index) => ({
    id: `project-${index + 1}`,
    name: `Project ${String(index + 1).padStart(2, '0')}`,
    created_at: '2026-08-01T00:00:00Z',
    updated_at: `2026-08-${String(28 - index).padStart(2, '0')}T00:00:00Z`,
  })
);

const SESSIONS: SessionListItem[] = Array.from({ length: 16 }, (_, index) => ({
  id: `session-${index + 1}`,
  workspace_id: index < 3 ? 'workspace-shared' : `workspace-${index + 1}`,
  execution_id: `task-${index + 1}`,
  project_id: `project-${(index % 4) + 1}`,
  task_id: `issue-${index + 1}`,
  title: `Session ${String(index + 1).padStart(2, '0')}`,
  executor: index % 2 === 0 ? 'Codex' : 'Claude Code',
  created_at: '2026-08-01T00:00:00Z',
  updated_at: `2026-08-${String(28 - index).padStart(2, '0')}T01:00:00Z`,
}));

type SourceMode = 'ready' | 'loading' | 'empty' | 'error' | 'degraded';

function updateTheme(theme: 'light' | 'dark') {
  document.documentElement.classList.remove('light', 'dark');
  document.documentElement.classList.add(theme);
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
}

function AppShellHarness() {
  const [fixtureLocale] = React.useState(() =>
    new URLSearchParams(window.location.search).get('locale')
  );
  const [dashboardOnly] = React.useState(
    () =>
      new URLSearchParams(window.location.search).get('view') === 'dashboard'
  );
  const [route, setRoute] = React.useState('/dashboard');
  const activeProjectId = route.match(/^\/projects\/([^/?#]+)/)?.[1] ?? null;
  const [activeSessionId, setActiveSessionId] = React.useState<string | null>(
    null
  );
  const [objectDrawerOpen, setObjectDrawerOpen] = React.useState(false);
  const [searchOpen, setSearchOpen] = React.useState(false);
  const [canvasMode, setCanvasMode] = React.useState<
    'contained' | 'full-bleed'
  >('contained');
  const [projects, setProjects] = React.useState(PROJECTS);
  const [projectHasNextPage, setProjectHasNextPage] = React.useState(false);
  const [projectPageLoads, setProjectPageLoads] = React.useState(0);
  const [projectSourceMode, setProjectSourceMode] =
    React.useState<SourceMode>('ready');
  const [sessionSourceMode, setSessionSourceMode] =
    React.useState<SourceMode>('ready');
  const [projectRetryCount, setProjectRetryCount] = React.useState(0);
  const [sessionRetryCount, setSessionRetryCount] = React.useState(0);
  const [showDashboardContract, setShowDashboardContract] =
    React.useState(false);
  const [systemAction, setSystemAction] = React.useState('none');
  const searchTriggerRef = React.useRef<HTMLElement | null>(null);
  const pageCanvasRef = React.useRef<HTMLElement>(null);
  const projectPageRequestedRef = React.useRef(false);

  const navigate = React.useCallback((nextRoute: string) => {
    setRoute(nextRoute);
    window.history.pushState({ fixture: 'app-shell' }, '', nextRoute);
    requestAnimationFrame(() => {
      pageCanvasRef.current?.focus({ preventScroll: true });
    });
  }, []);

  React.useEffect(() => {
    window.history.replaceState(
      { fixture: 'app-shell' },
      '',
      '/dashboard?fixture=app-shell'
    );
    updateTheme('light');
    if (
      fixtureLocale &&
      ['en', 'es', 'fr', 'ja', 'ko', 'zh-Hans', 'zh-Hant'].includes(
        fixtureLocale
      )
    ) {
      document.documentElement.dataset.fixtureLocale = fixtureLocale;
      void i18n.changeLanguage(fixtureLocale);
    }
  }, [fixtureLocale]);

  const adapter = React.useMemo<AppShellCapabilityAdapter>(
    () => ({
      deployment: 'local',
      discoveryHostId: 'fixture-host',
      discoveryScopeKey: '["local","fixture-host","fixture-user"]',
      environmentLabel: 'Fixture / Local',
      versionLabel: '0.1.0-contract',
      updateNotice: {
        phase: 'restart-ready',
        version: '2.0.0',
        open: () => {
          setSystemAction('update');
          navigate('/settings');
        },
      },
      userLabel: 'Fixture User',
      moduleCapabilities: {
        dashboard: {
          availability: 'available',
          navigate: () => navigate('/dashboard'),
        },
        projects: {
          availability: 'available',
          navigate: () => navigate('/projects'),
        },
        workflows: {
          availability: 'unavailable',
          reason: 'Workflow service is unavailable in this environment.',
        },
        agents: {
          availability: 'available',
          navigate: () => navigate('/agents'),
        },
      },
      navigateToRoute: navigate,
      openSettings: () => {
        setSystemAction('settings');
        navigate('/settings');
      },
      openUser: () => setSystemAction('user'),
    }),
    [navigate]
  );

  const requestProjectPage = React.useCallback(() => {
    if (projectPageRequestedRef.current) return;
    projectPageRequestedRef.current = true;
    setProjectPageLoads((count) => count + 1);
    setProjects((items) => [
      ...items,
      {
        id: 'project-loaded',
        name: 'Project loaded automatically',
        created_at: '2026-08-01T00:00:00Z',
        updated_at: '2026-07-01T00:00:00Z',
      },
    ]);
    setProjectHasNextPage(false);
  }, []);

  const enableProjectPaging = () => {
    projectPageRequestedRef.current = false;
    setProjectHasNextPage(true);
  };

  const visibleProjects =
    projectSourceMode === 'ready' || projectSourceMode === 'degraded'
      ? projects
      : [];
  const visibleSessions =
    sessionSourceMode === 'ready' || sessionSourceMode === 'degraded'
      ? SESSIONS
      : [];
  const retryProjects = React.useCallback(() => {
    setProjectRetryCount((count) => count + 1);
    setProjectSourceMode('ready');
  }, []);
  const retrySessions = React.useCallback(() => {
    setSessionRetryCount((count) => count + 1);
    setSessionSourceMode('ready');
  }, []);

  const projectState: SidebarSectionState<ProjectListItem> = {
    items: visibleProjects,
    isLoading: projectSourceMode === 'loading',
    isError: projectSourceMode === 'error' || projectSourceMode === 'degraded',
    hasNextPage: projectSourceMode === 'ready' && projectHasNextPage,
    isFetchingNextPage: false,
    retry: retryProjects,
    loadNextPage: requestProjectPage,
  };
  const sessionState: SidebarSectionState<SessionListItem> = {
    items: visibleSessions,
    isLoading: sessionSourceMode === 'loading',
    isError: sessionSourceMode === 'error' || sessionSourceMode === 'degraded',
    hasNextPage: false,
    isFetchingNextPage: false,
    retry: retrySessions,
    loadNextPage: () => undefined,
  };
  const searchSources = React.useMemo(
    () => [
      {
        id: 'projects' as const,
        state: deriveSearchSourceState(
          projectSourceMode === 'error' || projectSourceMode === 'degraded',
          visibleProjects.length
        ),
        retry: retryProjects,
      },
      {
        id: 'sessions' as const,
        state: deriveSearchSourceState(
          sessionSourceMode === 'error' || sessionSourceMode === 'degraded',
          visibleSessions.length
        ),
        retry: retrySessions,
      },
    ],
    [
      projectSourceMode,
      retryProjects,
      retrySessions,
      sessionSourceMode,
      visibleProjects.length,
      visibleSessions.length,
    ]
  );

  const openSearch = () => {
    searchTriggerRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setObjectDrawerOpen(false);
    setSearchOpen(true);
  };
  const closeSearch = (options?: { restoreFocus?: boolean }) => {
    setSearchOpen(false);
    if (options?.restoreFocus === false) return;
    requestAnimationFrame(() => {
      searchTriggerRef.current?.focus({ preventScroll: true });
    });
  };

  return (
    <div className="vk-app-shell" data-testid="app-shell">
      <div className="vk-app-shell__layout" data-search-open={searchOpen}>
        <ProductSidebar
          adapter={adapter}
          activeModule={deriveActiveShellModule(route.split('?')[0])}
          activeProjectId={activeProjectId}
          activeSessionId={activeSessionId}
          projects={projectState}
          sessions={sessionState}
          newSession={
            new URLSearchParams(window.location.search).has('noHost')
              ? {
                  availability: 'unavailable',
                  reason: 'Connect an online Host before starting a session.',
                }
              : {
                  availability: 'available',
                  navigate: () => {
                    setActiveSessionId(null);
                    navigate('/workspaces/create');
                  },
                }
          }
          objectDrawerOpen={objectDrawerOpen}
          onObjectDrawerOpenChange={setObjectDrawerOpen}
          onSearch={openSearch}
          onProject={(projectId) => {
            navigate(`/projects/${projectId}`);
          }}
          onSession={(session) => {
            setActiveSessionId(session.id);
            navigate(
              `/workspaces/${session.workspace_id}?session_id=${session.id}`
            );
          }}
          onDeleteSession={() => undefined}
        />
        <div className="vk-page-stack">
          <PageCanvas ref={pageCanvasRef} mode={dashboardOnly ? 'full-bleed' : canvasMode}>
            <section className={dashboardOnly ? 'fixture-dashboard-page' : 'fixture-page'}>
              {dashboardOnly ? (
                <DashboardPage />
              ) : (
                <>
                  <h1>App Shell browser contract</h1>
                  <p>
                    Deterministic fixture using the production shell components.
                  </p>
                  <div className="fixture-page__actions">
                    <button type="button" onClick={enableProjectPaging}>
                      Enable automatic project page
                    </button>
                    <button
                      type="button"
                      onClick={() => setCanvasMode('contained')}
                    >
                      Contained canvas
                    </button>
                    <button
                      type="button"
                      onClick={() => setCanvasMode('full-bleed')}
                    >
                      Full-bleed canvas
                    </button>
                    <button type="button" onClick={() => updateTheme('light')}>
                      Light theme
                    </button>
                    <button type="button" onClick={() => updateTheme('dark')}>
                      Dark theme
                    </button>
                    <button
                      type="button"
                      onClick={() => setProjectSourceMode('loading')}
                    >
                      Set projects loading
                    </button>
                    <button
                      type="button"
                      onClick={() => setProjectSourceMode('empty')}
                    >
                      Set projects empty
                    </button>
                    <button
                      type="button"
                      onClick={() => setProjectSourceMode('error')}
                    >
                      Set projects initial error
                    </button>
                    <button
                      type="button"
                      onClick={() => setProjectSourceMode('degraded')}
                    >
                      Set projects cached error
                    </button>
                    <button
                      type="button"
                      onClick={() => setSessionSourceMode('empty')}
                    >
                      Set sessions empty
                    </button>
                    <button
                      type="button"
                      onClick={() => setSessionSourceMode('error')}
                    >
                      Set sessions initial error
                    </button>
                    <button
                      type="button"
                      onClick={() => setSessionSourceMode('degraded')}
                    >
                      Set sessions cached error
                    </button>
                    <button
                      type="button"
                      onClick={() => setShowDashboardContract(true)}
                    >
                      Show dashboard state contract
                    </button>
                  </div>
                  <dl className="fixture-outputs">
                    <div>
                      <dt>Route</dt>
                      <dd data-testid="current-route">{route}</dd>
                    </div>
                    <div>
                      <dt>Automatic project page loads</dt>
                      <dd data-testid="project-page-loads">
                        {projectPageLoads}
                      </dd>
                    </div>
                    <div>
                      <dt>System action</dt>
                      <dd data-testid="system-action">{systemAction}</dd>
                    </div>
                    <div>
                      <dt>Canvas mode</dt>
                      <dd data-testid="canvas-mode">{canvasMode}</dd>
                    </div>
                    <div>
                      <dt>Project retries</dt>
                      <dd data-testid="project-retries">{projectRetryCount}</dd>
                    </div>
                    <div>
                      <dt>Session retries</dt>
                      <dd data-testid="session-retries">{sessionRetryCount}</dd>
                    </div>
                  </dl>
                  {showDashboardContract && <DashboardPage />}
                  <div className="fixture-wide-content" />
                </>
              )}
            </section>
          </PageCanvas>
        </div>
      </div>
      <GlobalSearchPalette
        open={searchOpen}
        scopeKey={adapter.discoveryScopeKey}
        projects={visibleProjects}
        sessions={visibleSessions}
        sources={searchSources}
        moduleCapabilities={adapter.moduleCapabilities}
        onClose={closeSearch}
        onNavigate={(nextRoute) => {
          navigate(nextRoute);
          return true;
        }}
      />
    </div>
  );
}

const root = document.getElementById('root');
if (!root) throw new Error('App Shell fixture root is missing');
createRoot(root).render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><AppShellHarness /></QueryClientProvider>);
