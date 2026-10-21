import {
  createContext,
  useContext,
  useState,
  useEffect,
  type ReactNode,
} from "react";
export * from "../../../packages/web-core/src/shared/lib/remoteApi";
export * from "../../../packages/web-core/src/shared/lib/electric/collections";

export const isLocalRemoteApiEnabled = () =>
  new URLSearchParams(location.search).has("local");
export function refreshShapeFallback(
  _shape: unknown,
  params: unknown,
  hostId: string | null,
) {
  record("refreshes", { params, hostId });
}
export async function makeLocalApiRequest(
  path: string,
  options: { body?: string; hostId?: string | null } = {},
) {
  record("requests", { path, ...options });
  if (path === "/api/local/v1/projects/open") {
    const input = JSON.parse(options.body ?? "{}");
    record("opens", { input, hostId: options.hostId });
    const query = new URLSearchParams(location.search);
    const existing = {
      id: "68916c8e-7f85-48f1-b15e-a63cb87f4e45",
      organization_id: "org-fixture",
      name: "Portable project",
      color: "100 20% 30%",
    };
    if (query.has("ordinary-conflict")) {
      return Response.json(
        { success: false, message: "The directory is mounted elsewhere." },
        { status: 409 },
      );
    }
    if (query.has("text-conflict")) {
      return new Response("PROJECT_DIRECTORY_EXISTS", { status: 409 });
    }
    if (query.has("legacy-conflict")) {
      return Response.json(
        {
          success: false,
          data: null,
          message: "Project directory already exists",
          error_data: {
            code: "PROJECT_DIRECTORY_EXISTS",
            project: existing,
          },
        },
        { status: 409 },
      );
    }
    if (query.has("hold-open")) {
      await new Promise<void>((resolve) =>
        window.addEventListener("fixture-open-release", () => resolve(), {
          once: true,
        }),
      );
    }
    // Even when detached metadata exists, create returns the new draft's
    // identity and fields. Recovery is not a supported server operation.
    if (query.has("detached-project")) record("detachedProject", existing);
    return Response.json({
      success: true,
      data: {
        id: "directory-project",
        organization_id: "org-fixture",
        name: input.name,
        color: input.color,
      },
      txid: 1,
    });
  }
  return new Response(JSON.stringify({ tasks: [], data: {}, templates: [] }), {
    status: 200,
  });
}

function record(key: string, value: unknown) {
  const data = document.documentElement.dataset;
  data[key] = JSON.stringify([...JSON.parse(data[key] ?? "[]"), value]);
}

export function useShape() {
  return {
    insert: (input: unknown) => {
      record("inserts", input);
      const project = { id: "created-project", ...(input as object) };
      return { data: project, persisted: Promise.resolve(project) };
    },
    remove: (id: string) => {
      record("removes", id);
      const fails =
        new URLSearchParams(location.search).has("cancel-fail") &&
        JSON.parse(document.documentElement.dataset.removes!).length === 1;
      return {
        persisted: fails
          ? Promise.reject(new Error("Simulated delete failure"))
          : Promise.resolve(),
      };
    },
    error: null,
  };
}

const hosts = [
  { id: "local", apiHostId: null, label: "This machine", kind: "local" },
  {
    id: "remote-1",
    apiHostId: "remote-1",
    label: "Remote machine",
    kind: "remote",
    status: "online",
  },
];
const HostContext = createContext({
  selectedHostId: "local",
  setSelectedHostId: (_: string) => {},
});
export function SettingsHostProvider({ children }: { children: ReactNode }) {
  const [selectedHostId, setSelectedHostId] = useState("local");
  useEffect(() => {
    const onSwitch = () => setSelectedHostId("local");
    window.addEventListener("fixture-host-switch", onSwitch);
    return () => window.removeEventListener("fixture-host-switch", onSwitch);
  }, []);
  return (
    <HostContext.Provider value={{ selectedHostId, setSelectedHostId }}>
      {children}
    </HostContext.Provider>
  );
}
export function useSettingsHost() {
  const context = useContext(HostContext);
  const [offline, setOffline] = useState(false);
  useEffect(() => {
    const onOffline = () => setOffline(true);
    window.addEventListener("fixture-host-offline", onOffline);
    return () => window.removeEventListener("fixture-host-offline", onOffline);
  }, []);
  const selectedHost = hosts.find((h) => h.id === context.selectedHostId);
  return {
    ...context,
    selectedHost:
      offline && selectedHost?.kind === "remote"
        ? { ...selectedHost, status: "offline" }
        : selectedHost,
    availableHosts: hosts,
  };
}

export const WorkspaceTargetDialog = {
  async show(options: unknown) {
    record("picker", options);
    if (new URLSearchParams(location.search).has("picker-cancel"))
      return { kind: "canceled" };
    const git = new URLSearchParams(location.search).get("mode") === "git";
    return {
      kind: "confirmed",
      selection: git
        ? {
            mode: "worktree",
            path: "/fixture/repo",
            repo: { id: "repo-2" },
            targetBranch: "feature/test",
          }
        : { mode: "direct_folder", path: "/fixture/folder" },
    };
  },
};

export const projectWorkspaceDefaultQueryKey = (
  id: string,
  hostId: string | null,
) => ["project-default", id, hostId];
export async function saveProjectWorkspaceDefault(
  projectId: string,
  value: unknown,
  hostId: string | null,
) {
  record("saves", { projectId, value, hostId });
  if (new URLSearchParams(location.search).has("hold")) {
    await new Promise<void>((resolve) =>
      window.addEventListener("fixture-save-release", () => resolve(), {
        once: true,
      }),
    );
  }
  if (
    new URLSearchParams(location.search).has("fail") &&
    JSON.parse(document.documentElement.dataset.saves!).length === 1
  ) {
    throw new Error("Simulated save failure");
  }
}
