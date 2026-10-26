import { useLayoutEffect } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createRootRoute,
  createRoute,
  createRouter,
  Link,
  Outlet,
  RouterProvider,
  useLocation,
} from "@tanstack/react-router";
import { useRenderedPathname } from "@/shared/hooks/useRenderedPathname";
import { useCurrentAppDestination } from "@/shared/hooks/useCurrentAppDestination";
import { AppNavigationProvider } from "@/shared/hooks/useAppNavigation";
import {
  WorkspaceContext,
  type WorkspaceContextValue,
} from "@/shared/hooks/useWorkspaceContext";
import {
  appShellDiscoveryQueryKey,
  derivePageCanvasMode,
  type AppShellCapabilityAdapter,
} from "@/features/app-shell/model/appShell";
import { AppShellContainer } from "@/features/app-shell/containers/AppShellContainer";
import type { AppNavigation } from "@/shared/lib/routes/appNavigation";
import "@/i18n/config";
import "../../../packages/ui/src/styles/tokens.css";
import "../../app-shell/fixture/src/style.css";

type Observation = {
  page: string;
  pathname: string;
  mode: string;
  missing: boolean;
};
declare global {
  interface Window {
    routeTest: {
      observations: Observation[];
      arm(): void;
      release(): void;
    };
  }
}
const observations: Observation[] = [];
let release: (() => void) | undefined;
let settingsGate: Promise<void> | undefined;

Object.assign(window, {
  routeTest: {
    observations,
    arm() {
      settingsGate = new Promise<void>((resolve) => {
        release = resolve;
      });
    },
    release() {
      release?.();
      settingsGate = undefined;
    },
  },
});

// Cache canonical discovery rows; the real shell and router derive selection.
const queryClient = new QueryClient();
const scopeKey = "route-transition";
queryClient.setQueryData(appShellDiscoveryQueryKey(scopeKey, "projects"), {
  pages: [
    {
      projects: [
        {
          id: "one",
          name: "Project One",
          created_at: "2026-10-01T00:00:00Z",
          updated_at: "2026-10-01T00:00:00Z",
        },
      ],
      next_cursor: null,
    },
  ],
  pageParams: [null],
});
queryClient.setQueryData(appShellDiscoveryQueryKey(scopeKey, "sessions"), {
  pages: [
    {
      sessions: ["one", "two"].map((id) => ({
        id: `session-${id}`,
        workspace_id: `workspace-${id}`,
        execution_id: `execution-${id}`,
        project_id: "one",
        task_id: "task-one",
        title: `Session ${id}`,
        executor: "Codex",
        created_at: "2026-10-01T00:00:00Z",
        updated_at: "2026-10-01T00:00:00Z",
      })),
      next_cursor: null,
    },
  ],
  pageParams: [null],
});

const noop = () => {};
const asyncNoop = async () => {};
// The provider deliberately survives route changes, as in the real app shell.
const retainedWorkspace: WorkspaceContextValue = {
  workspaceId: "workspace-one",
  workspace: undefined,
  activeWorkspaces: [],
  archivedWorkspaces: [],
  workspaceListState: "ready",
  isWorkspacesListLoading: false,
  isWorkspacesListRetrying: false,
  workspaceListError: null,
  retryWorkspaces: asyncNoop,
  isLoading: false,
  isWorkspaceLoading: false,
  workspaceError: null,
  retryWorkspace: asyncNoop,
  isCreateMode: false,
  selectWorkspace: noop,
  navigateToCreate: noop,
  sessions: [],
  selectedSession: undefined,
  selectedSessionId: "session-one",
  selectSession: noop,
  selectLatestSession: noop,
  isSessionsLoading: false,
  sessionsError: null,
  retrySessions: asyncNoop,
  isNewSessionMode: false,
  startNewSession: noop,
  repos: [],
  isReposLoading: false,
  reposError: null,
  retryRepos: asyncNoop,
};

const navigation: AppNavigation = {
  resolveFromPath(path: string) {
    const { foundRoute, routeParams } = router.getMatchedRoutes(path);
    if (foundRoute?.id === project.id)
      return { kind: "project", projectId: routeParams.projectId };
    if (foundRoute?.id === projectWorkflows.id)
      return { kind: "project-workflows", projectId: routeParams.projectId };
    if (foundRoute?.id === workflowEdit.id)
      return {
        kind: "project-workflow-edit",
        projectId: routeParams.projectId,
        workflowId: routeParams.workflowId,
      };
    if (foundRoute?.id === workflowRun.id)
      return {
        kind: "project-workflow-run",
        projectId: routeParams.projectId,
        runId: routeParams.runId,
      };
    if (foundRoute?.id === taskWorkspace.id)
      return {
        kind: "project-task-workspace",
        projectId: routeParams.projectId,
        taskId: routeParams.taskId,
        workspaceId: routeParams.workspaceId,
      };
    if (foundRoute?.id === workspace.id)
      return { kind: "workspace", workspaceId: routeParams.workspaceId };
    if (foundRoute?.id === workspaceCreate.id)
      return { kind: "workspaces-create" };
    return null;
  },
  goToRoot: noop,
  goToOnboarding: noop,
  goToOnboardingSignIn: noop,
  goToWorkspaces: noop,
  goToWorkspacesCreate: noop,
  goToWorkspace: noop,
  goToWorkspaceVsCode: noop,
  goToExport: noop,
  goToProject: (projectId) => {
    void router.navigate({ to: "/projects/$projectId", params: { projectId } });
  },
  goToProjectWorkflows: noop,
  goToProjectWorkflowEdit: noop,
  goToProjectWorkflowRun: noop,
  goToProjectTask: noop,
  goToProjectTaskWorkspace: noop,
  goToProjectTaskWorkspaceCreate: noop,
  goToProjectWorkspaceCreate: noop,
};

function navigateToRoute(href: string) {
  void router.navigate({ href });
}
const adapter: AppShellCapabilityAdapter = {
  deployment: "local",
  discoveryHostId: null,
  discoveryScopeKey: scopeKey,
  environmentLabel: "Route fixture",
  moduleCapabilities: {
    dashboard: {
      availability: "available",
      navigate: () => navigateToRoute("/dashboard"),
    },
    projects: {
      availability: "available",
      navigate: () => navigateToRoute("/projects"),
    },
    workflows: {
      availability: "available",
      navigate: () => navigateToRoute("/workflows"),
    },
    agents: {
      availability: "available",
      navigate: () => navigateToRoute("/agents"),
    },
  },
  navigateToRoute,
  openSettings: () => navigateToRoute("/settings"),
};

function Page({ name }: { name: string }) {
  const destination = useCurrentAppDestination();
  const pathname = useRenderedPathname();
  const missing = name === "project" && destination?.kind !== "project";
  useLayoutEffect(() => {
    observations.push({
      page: name,
      pathname,
      mode: derivePageCanvasMode(pathname),
      missing,
    });
  });
  return (
    <section data-testid="page" data-page={name}>
      {missing ? "Project missing" : name}
    </section>
  );
}

function Shell() {
  const location = useLocation();
  const pathname = useRenderedPathname();
  return (
    <AppNavigationProvider value={navigation}>
      <WorkspaceContext.Provider value={retainedWorkspace}>
        <AppShellContainer adapter={adapter}>
          <nav>
            <Link to="/projects/$projectId" params={{ projectId: "one" }}>
              Project
            </Link>{" "}
            <Link
              to="/projects/$projectId/tasks/$taskId/workspaces/$workspaceId"
              params={{
                projectId: "one",
                taskId: "task-one",
                workspaceId: "workspace-one",
              }}
            >
              Task
            </Link>{" "}
            <Link
              to="/projects/$projectId/workflows/$workflowId/edit"
              params={{ projectId: "one", workflowId: "workflow-one" }}
            >
              Edit workflow
            </Link>{" "}
            <Link
              to="/projects/$projectId/workflow-runs/$runId"
              params={{ projectId: "one", runId: "run-one" }}
            >
              Workflow run
            </Link>{" "}
            <Link to="/settings">Settings</Link>{" "}
            <Link to="/dashboard">Dashboard</Link>
          </nav>
          <output data-testid="location">{location.pathname}</output>
          <output data-testid="rendered">{pathname}</output>
          <div data-testid="canvas" data-mode={derivePageCanvasMode(pathname)}>
            <Outlet />
          </div>
        </AppShellContainer>
      </WorkspaceContext.Provider>
    </AppNavigationProvider>
  );
}

const root = createRootRoute({ component: Shell });
const project = createRoute({
  getParentRoute: () => root,
  path: "/projects/$projectId",
  component: () => <Page name="project" />,
});
const taskWorkspace = createRoute({
  getParentRoute: () => root,
  path: "/projects/$projectId/tasks/$taskId/workspaces/$workspaceId",
  component: () => <Page name="task" />,
});
const projectWorkflows = createRoute({
  getParentRoute: () => root,
  path: "/projects/$projectId/workflows",
  component: () => <Page name="project-workflows" />,
});
const workflowEdit = createRoute({
  getParentRoute: () => root,
  path: "/projects/$projectId/workflows/$workflowId/edit",
  component: () => <Page name="workflow-edit" />,
});
const workflowRun = createRoute({
  getParentRoute: () => root,
  path: "/projects/$projectId/workflow-runs/$runId",
  component: () => <Page name="workflow-run" />,
});
const workspace = createRoute({
  getParentRoute: () => root,
  path: "/workspaces/$workspaceId",
  component: () => <Page name="session" />,
});
const workspaceCreate = createRoute({
  getParentRoute: () => root,
  path: "/workspaces/create",
  component: () => <Page name="create" />,
});
const projects = createRoute({
  getParentRoute: () => root,
  path: "/projects",
  component: () => <Page name="projects" />,
});
const agents = createRoute({
  getParentRoute: () => root,
  path: "/agents",
  component: () => <Page name="agents" />,
});
const workflows = createRoute({
  getParentRoute: () => root,
  path: "/workflows",
  component: () => <Page name="workflows" />,
});
const settings = createRoute({
  getParentRoute: () => root,
  path: "/settings",
  loader: () => settingsGate,
  component: () => <Page name="settings" />,
});
const dashboard = createRoute({
  getParentRoute: () => root,
  path: "/dashboard",
  component: () => <Page name="dashboard" />,
});
const router = createRouter({
  routeTree: root.addChildren([
    project,
    taskWorkspace,
    projectWorkflows,
    workflowEdit,
    workflowRun,
    workspace,
    workspaceCreate,
    projects,
    agents,
    workflows,
    settings,
    dashboard,
  ]),
  defaultPendingMs: 60_000,
  defaultPreload: false,
});

createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={queryClient}>
    <RouterProvider router={router} />
  </QueryClientProvider>,
);
