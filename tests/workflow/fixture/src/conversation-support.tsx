import type { ReactNode } from "react";
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import { HotkeysProvider } from "react-hotkeys-hook";
import { Provider as NiceModalProvider } from "@ebay/nice-modal-react";
import {
  UserSystemContext,
  type UserSystemContextType,
  type UserSystemState,
} from "@/shared/hooks/useUserSystem";
import { AppRuntimeProvider } from "@/shared/hooks/useAppRuntime";
import { HostIdProvider } from "@/shared/providers/HostIdProvider";
import { TerminalProvider } from "@/shared/providers/TerminalProvider";
import { useScratch } from "@/shared/hooks/useScratch";
import { ScratchType } from "shared/types";

/** Exercise persisted Host/Session identity independently of Agent discovery. */
export function ScratchScopeFixture() {
  const { scratch, isLoading, updateScratch, deleteScratch } = useScratch(
    ScratchType.DRAFT_FOLLOW_UP,
    "same-session",
  );
  return (
    <>
      <textarea
        aria-label="Session draft"
        disabled={isLoading}
        value={
          scratch?.payload.type === "DRAFT_FOLLOW_UP"
            ? scratch.payload.data.message
            : ""
        }
        onChange={(event) =>
          void updateScratch({
            payload: {
              type: "DRAFT_FOLLOW_UP",
              data: {
                message: event.target.value,
                executor_config: { executor: "CODEX" },
              },
            },
          })
        }
      />
      <button onClick={() => void deleteScratch()}>Delete draft</button>
    </>
  );
}

const system: UserSystemState = {
  appVersion: "test",
  previewProxyPort: null,
  config: null,
  environment: null,
  profiles: null,
  capabilities: null,
  machineId: "fixture",
  loginStatus: null,
  remoteAuthDegraded: null,
};
const userSystem: UserSystemContextType = {
  ...system,
  system,
  loading: false,
  updateConfig: () => {},
  updateAndSaveConfig: async () => true,
  saveConfig: async () => true,
  reloadSystem: async () => {},
  setEnvironment: () => {},
  setProfiles: () => {},
  setCapabilities: () => {},
};

/** Real chat dependencies; network and process streams are supplied by tests. */
export function createConversationFixtureRouter(children: ReactNode) {
  const runtime =
    new URLSearchParams(location.search).get("runtime") === "remote"
      ? "remote"
      : "local";
  const root = createRootRoute({
    component: () => (
      <AppRuntimeProvider runtime={runtime}>
        <UserSystemContext.Provider value={userSystem}>
          <HostIdProvider>
            <NiceModalProvider>
              <HotkeysProvider initiallyActiveScopes={["global", "workspace"]}>
                <TerminalProvider>{children}</TerminalProvider>
              </HotkeysProvider>
            </NiceModalProvider>
          </HostIdProvider>
        </UserSystemContext.Provider>
      </AppRuntimeProvider>
    ),
  });
  return createRouter({
    routeTree: root,
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
}

export { RouterProvider };
