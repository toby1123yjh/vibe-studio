import * as React from "react";
import NiceModal from "@ebay/nice-modal-react";
import { HotkeysProvider } from "react-hotkeys-hook";
import { createRoot } from "react-dom/client";
import { useQueryClient } from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  BaseCodingAgent,
  EditorType,
  MemberRole,
  SoundFile,
  ThemeMode,
  type Config,
  type UserSystemInfo,
} from "shared/types";
import { AppNavigationProvider } from "../../../../packages/web-core/src/shared/hooks/useAppNavigation";
import { AuthContext } from "../../../../packages/web-core/src/shared/hooks/auth/useAuth";
import { AppRuntimeProvider } from "../../../../packages/web-core/src/shared/hooks/useAppRuntime";
import { AppShellProjectsProvider } from "../../../../packages/web-core/src/shared/hooks/useAppShellProjects";
import { SettingsPage } from "../../../../packages/web-core/src/features/settings/ui/SettingsPage";
import { SettingsDirtyProvider } from "../../../../packages/web-core/src/shared/dialogs/settings/settings/SettingsDirtyContext";
import { SettingsHostProvider } from "../../../../packages/web-core/src/shared/dialogs/settings/settings/SettingsHostContext";
import { SettingsMachineUserSystemProvider } from "../../../../packages/web-core/src/shared/dialogs/settings/settings/SettingsMachineUserSystemProvider";
import { organizationKeys } from "../../../../packages/web-core/src/shared/hooks/organizationKeys";
import { configureAuthRuntime } from "../../../../packages/web-core/src/shared/lib/auth/runtime";
import { setLocalRemoteApiEnabled } from "../../../../packages/web-core/src/shared/lib/remoteApi";
import { useAppUpdateStore } from "../../../../packages/web-core/src/shared/stores/useAppUpdateStore";
import { ProjectDirectoryPage } from "../../../../packages/web-core/src/features/projects/ui/ProjectDirectoryPage";
import { DefaultProjectPage } from "../../../../packages/web-core/src/features/projects/ui/ProjectSessions";
import { ProjectKanbanView } from "../../../../packages/web-core/src/features/projects/ui/ProjectKanbanView";
import { ProjectBoardActions } from "../../../../packages/web-core/src/features/projects/ui/ProjectBoardActions";
import { useFixtureProjects } from "./projectSettingsMock";
import { WorkspacesSidebar } from "../../../../packages/ui/src/components/WorkspacesSidebar";
import "../../../../packages/web-core/src/i18n/config";
import "../../../../packages/ui/src/styles/tokens.css";
import "./style.css";

setLocalRemoteApiEnabled(true);
configureAuthRuntime({
  getToken: async () => "page-surfaces-fixture-token",
  triggerRefresh: async () => "page-surfaces-fixture-token",
  registerShape: () => () => undefined,
  getCurrentUser: async () => ({ user_id: "fixture-user" }),
});

const workspaces = [
  {
    id: "workspace-1",
    name: "Fix keyboard navigation",
    isRunning: true,
    isPinned: true,
    hasPendingApproval: true,
  },
  {
    id: "workspace-2",
    name: "Add project metrics",
    isRunning: false,
    isPinned: false,
    hasUnseenActivity: true,
  },
  {
    id: "workspace-3",
    name: "Update release notes",
    isRunning: false,
    isPinned: false,
  },
];

function navigation() {
  const record = (destination: string) => {
    window.dispatchEvent(
      new CustomEvent("fixture-navigation", { detail: destination }),
    );
  };
  return {
    resolveFromPath: () => null,
    goToRoot: () => record("root"),
    goToOnboarding: () => record("onboarding"),
    goToOnboardingSignIn: () => record("onboarding-sign-in"),
    goToWorkspaces: () => record("workspaces"),
    goToWorkspacesCreate: () => record("workspaces-create"),
    goToWorkspace: (id: string) => record(`workspace:${id}`),
    goToWorkspaceVsCode: (id: string) => record(`vscode:${id}`),
    goToExport: () => record("export"),
    goToProject: (id: string) => record(`project:${id}`),
    goToProjectWorkflows: () => record("project-workflows"),
    goToProjectWorkflowEdit: () => record("project-workflow-edit"),
    goToProjectWorkflowRun: () => record("project-workflow-run"),
    goToProjectTask: () => record("project-task"),
    goToProjectTaskArena: () => record("project-task-arena"),
    goToProjectTaskWorkspace: () => record("project-task-workspace"),
    goToProjectTaskWorkspaceCreate: () =>
      record("project-task-workspace-create"),
    goToProjectWorkspaceCreate: (projectId: string) =>
      record(`project-workspace-create:${projectId}`),
  };
}

function ProjectDirectoryFixture({ board = false }: { board?: boolean }) {
  const projects = useFixtureProjects();
  const [taskComposerRequests, setTaskComposerRequests] = React.useState(0);
  const [taskComposerStatus, setTaskComposerStatus] = React.useState("");
  const openTaskComposer = (statusId?: string) => {
    setTaskComposerStatus(statusId ?? "");
    setTaskComposerRequests((count) => count + 1);
  };
  const [scope, setScope] = React.useState("fixture");
  React.useEffect(() => {
    const changeScope = () => setScope((previous) => `${previous}-changed`);
    window.addEventListener("fixture-change-scope", changeScope);
    return () =>
      window.removeEventListener("fixture-change-scope", changeScope);
  }, []);
  const projectsState = React.useMemo(
    () => ({
      scopeKey: scope,
      deployment: "local" as const,
      hostId: null,
      items: projects,
      isLoading: false,
      isError: false,
      isFetching: false,
      isFetchNextPageError: false,
      hasNextPage: false,
      isFetchingNextPage: false,
      retry: async () => {
        document.documentElement.dataset.projectRefreshes = String(
          Number(document.documentElement.dataset.projectRefreshes ?? 0) + 1,
        );
      },
      loadNextPage: async () => undefined,
    }),
    [projects, scope],
  );
  return (
    <AppShellProjectsProvider value={projectsState}>
      <section data-testid="project-directory-surface" className="fixture-page">
        <output data-testid="issue-composer-requests" hidden>
          {taskComposerRequests}
        </output>
        <output data-testid="task-composer-status" hidden>
          {taskComposerStatus}
        </output>
        {new URLSearchParams(location.search).has("defaultProject") ? (
          <DefaultProjectPage />
        ) : board ? (
          <ProjectKanbanView
            projectName="Project 2"
            projectActions={
              projects.some((project) => project.id === "project-2") ? (
                <ProjectBoardActions
                  projectId="project-2"
                  projectName="Project 2"
                  hostId={null}
                />
              ) : null
            }
            columns={
              new URLSearchParams(location.search).has("taskColumns")
                ? ["Discuss", "Todo", "In Progress", "In Review", "Done"].map(
                    (name, index) => ({
                      id: `status-${index}`,
                      name,
                      color: "210 80% 52%",
                      sortOrder: index,
                      tasks: [],
                    }),
                  )
                : []
            }
            taskCount={0}
            query=""
            selectedTaskId={null}
            dragDisabled={false}
            executionSource={{ state: "ready" }}
            panel={null}
            onQueryChange={() => undefined}
            onCreateTask={openTaskComposer}
            onOpenTask={() => undefined}
            onOpenExecution={() => undefined}
            onDeleteTask={async () => undefined}
            getExecutionUnavailableReason={() => null}
            onMove={async () => undefined}
          />
        ) : (
          <ProjectDirectoryPage />
        )}
      </section>
    </AppShellProjectsProvider>
  );
}

function WorkspaceListFixture() {
  const [selected, setSelected] = React.useState<string | null>(null);
  const [query, setQuery] = React.useState("");
  return (
    <section
      data-testid="workspace-list-surface"
      className="fixture-page fixture-workspaces"
    >
      <WorkspacesSidebar
        workspaces={workspaces}
        totalWorkspacesCount={workspaces.length}
        selectedWorkspaceId={selected}
        onSelectWorkspace={setSelected}
        onAddWorkspace={() => undefined}
        searchQuery={query}
        onSearchChange={setQuery}
        layoutMode="accordion"
        onToggleLayoutMode={() => undefined}
        onOpenWorkspaceActions={() => undefined}
        listState="ready"
      />
      <output data-testid="selected-workspace">{selected ?? ""}</output>
    </section>
  );
}

const settingsConfig: Config = {
  config_version: "fixture",
  theme: ThemeMode.SYSTEM,
  executor_profile: { executor: BaseCodingAgent.CODEX, variant: null },
  disclaimer_acknowledged: true,
  onboarding_acknowledged: true,
  remote_onboarding_acknowledged: true,
  notifications: {
    sound_enabled: false,
    push_enabled: false,
    sound_file: SoundFile.ABSTRACT_SOUND1,
  },
  editor: {
    editor_type: EditorType.VS_CODE,
    custom_command: null,
    remote_ssh_host: null,
    remote_ssh_user: null,
    auto_install_extension: true,
  },
  github: {
    pat: null,
    oauth_token: null,
    username: null,
    primary_email: null,
    default_pr_base: null,
  },
  analytics_enabled: false,
  workspace_dir: null,
  last_app_version: null,
  show_release_notes: false,
  language: "EN",
  git_branch_prefix: "",
  showcases: { seen_features: [] },
  pr_auto_description_enabled: false,
  pr_auto_description_prompt: null,
  commit_reminder_enabled: false,
  commit_reminder_prompt: null,
  send_message_shortcut: "ModifierEnter",
  relay_enabled: false,
  host_nickname: null,
  hidden_agents: [],
};

const settingsUserSystem: UserSystemInfo = {
  version: "0.1.44",
  config: settingsConfig,
  machine_id: "fixture-machine",
  login_status: { status: "loggedout" },
  remote_auth_degraded: null,
  environment: {
    os_type: "fixture",
    os_version: "1",
    os_architecture: "x64",
    bitness: "64",
  },
  capabilities: {},
  shared_api_base: null,
  preview_proxy_port: null,
  executors: {},
};

function SettingsFixture() {
  const mode = new URLSearchParams(window.location.search).get("mode");
  const initialHostId = mode === "offline" ? "remote-1" : undefined;
  const queryClient = useQueryClient();
  const [search, setSearch] = React.useState({
    tab: mode === "actions" ? "cloud" : mode === "offline" ? "host" : "general",
    section:
      mode === "actions"
        ? "projects"
        : mode === "offline"
          ? "repositories"
          : "application",
    ...(mode === "actions" ? { projectId: "project-2" } : {}),
    ...(initialHostId ? { host: initialHostId } : {}),
  });
  const reportAvailable = useAppUpdateStore((state) => state.reportAvailable);
  const reportReady = useAppUpdateStore((state) => state.reportReady);

  return (
    <AppRuntimeProvider runtime="local">
      <SettingsDirtyProvider>
        <SettingsHostProvider initialHostId={initialHostId}>
          <SettingsMachineUserSystemProvider>
            <section
              data-testid="settings-surface"
              className="fixture-page fixture-settings"
            >
              <div
                className="fixture-settings-controls"
                aria-label="Fixture update controls"
              >
                <button
                  type="button"
                  data-testid="report-update"
                  onClick={() =>
                    reportAvailable(
                      "0.1.45",
                      "Keyboard and responsive improvements",
                    )
                  }
                >
                  Report update
                </button>
                <button
                  type="button"
                  data-testid="report-ready"
                  onClick={() => reportReady("0.1.45", async () => undefined)}
                >
                  Mark update ready
                </button>
                {mode === "degraded" && (
                  <button
                    type="button"
                    data-testid="refresh-host-sources"
                    onClick={() =>
                      void queryClient.invalidateQueries({
                        queryKey: ["user-system", "local"],
                      })
                    }
                  >
                    Refresh host sources
                  </button>
                )}
              </div>
              <SettingsPage
                search={search}
                onSearchChange={(next) => setSearch(next)}
              />
            </section>
          </SettingsMachineUserSystemProvider>
        </SettingsHostProvider>
      </SettingsDirtyProvider>
    </AppRuntimeProvider>
  );
}

function Harness() {
  const [surface, setSurface] = React.useState<
    "projects" | "workspaces" | "settings" | "board"
  >(
    () =>
      (new URLSearchParams(window.location.search).get("surface") as
        | "settings"
        | "board") ?? "projects",
  );
  return (
    <div className="fixture-shell">
      <nav aria-label="Surface selector" className="fixture-nav">
        <button
          type="button"
          aria-pressed={surface === "projects"}
          onClick={() => setSurface("projects")}
        >
          Projects
        </button>
        <button
          type="button"
          aria-pressed={surface === "workspaces"}
          onClick={() => setSurface("workspaces")}
        >
          Workspaces
        </button>
        <button
          type="button"
          aria-pressed={surface === "settings"}
          onClick={() => setSurface("settings")}
        >
          Settings
        </button>
      </nav>
      {surface === "projects" ? (
        <ProjectDirectoryFixture />
      ) : surface === "board" ? (
        <ProjectDirectoryFixture board />
      ) : surface === "workspaces" ? (
        <WorkspaceListFixture />
      ) : (
        <SettingsFixture />
      )}
    </div>
  );
}

const rootRoute = createRootRoute({ component: Harness });
const router = createRouter({
  routeTree: rootRoute,
  history: createMemoryHistory({ initialEntries: ["/"] }),
});
const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false } },
});
queryClient.setQueryData(organizationKeys.userList(), {
  organizations: [
    {
      id: "org-1",
      name: "Fixture Organization",
      slug: "fixture-organization",
      is_personal: true,
      issue_prefix: "FIX",
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T00:00:00Z",
      user_role: MemberRole.ADMIN,
    },
  ],
});

const root = document.getElementById("root");
if (!root) throw new Error("Page surfaces fixture root is missing");
createRoot(root).render(
  <QueryClientProvider client={queryClient}>
    <AuthContext.Provider
      value={{ isSignedIn: true, isLoaded: true, userId: "fixture-user" }}
    >
      <AppNavigationProvider value={navigation()}>
        <HotkeysProvider>
          <NiceModal.Provider>
            <RouterProvider router={router} />
          </NiceModal.Provider>
        </HotkeysProvider>
      </AppNavigationProvider>
    </AuthContext.Provider>
  </QueryClientProvider>,
);

router.subscribe("onResolved", () => {
  document.documentElement.dataset.fixtureRoute = router.state.location.href;
});
