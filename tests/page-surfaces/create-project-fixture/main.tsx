import React from "react";
import { createRoot } from "react-dom/client";
import NiceModal from "@ebay/nice-modal-react";
import { HotkeysProvider } from "react-hotkeys-hook";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { CreateRemoteProjectDialog } from "../../../packages/web-core/src/shared/dialogs/org/CreateRemoteProjectDialog";
import { DeleteRemoteProjectDialog } from "../../../packages/web-core/src/shared/dialogs/org/DeleteRemoteProjectDialog";
import i18n from "../../../packages/web-core/src/i18n/config";
import "../../../packages/ui/src/styles/tokens.css";
import "../fixture/src/style.css";

void i18n.changeLanguage("en");

function Fixture() {
  return (
    <>
      <button
        onClick={() => {
          void CreateRemoteProjectDialog.show({
            organizationId: "org-fixture",
          }).then((result) => {
            document.documentElement.dataset.result = JSON.stringify(result);
          });
        }}
      >
        Open create project
      </button>
      <button
        onClick={() => {
          void DeleteRemoteProjectDialog.show({
            projectName: "Portable project",
            onDelete: async () => {
              document.documentElement.dataset.deletionRequested = "true";
            },
          });
        }}
      >
        Open delete project
      </button>
    </>
  );
}

createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={new QueryClient()}>
    <HotkeysProvider initiallyActiveScopes={["global"]}>
      <NiceModal.Provider>
        <Fixture />
      </NiceModal.Provider>
    </HotkeysProvider>
  </QueryClientProvider>,
);
