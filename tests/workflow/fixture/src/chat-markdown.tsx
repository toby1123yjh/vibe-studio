import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ThemeMode } from "shared/types";
import i18n from "../../../../packages/web-core/src/i18n/config";
import "../../../../packages/web-core/src/app/styles/new/index.css";
import "../../../../packages/ui/src/styles/tokens.css";
import { ThemeProvider } from "@/shared/providers/ThemeProvider";
import { AppNavigationProvider } from "@/shared/hooks/useAppNavigation";
import type { AppNavigation } from "@/shared/lib/routes/appNavigation";
import { ChangesViewActionsContext } from "@/shared/hooks/useChangesView";
import { AppChatMarkdown } from "@/features/workspace-chat/ui/AppChatMarkdown";
import WYSIWYGEditor from "@/shared/components/WYSIWYGEditor";
import {
  createConversationFixtureRouter,
  RouterProvider,
} from "./conversation-support";

declare global {
  interface Window {
    chatMarkdownFixture: {
      replace(content: string): void;
      setHistory(count: number): void;
      startStream(): void;
      selectedPaths: string[];
    };
  }
}

void i18n.changeLanguage("en");
const selectedPaths: string[] = [];
const unexpectedNavigation = () => {
  throw new Error("Markdown must not navigate");
};
const navigation: AppNavigation = {
  resolveFromPath: () => null,
  goToRoot: unexpectedNavigation,
  goToOnboarding: unexpectedNavigation,
  goToOnboardingSignIn: unexpectedNavigation,
  goToWorkspaces: unexpectedNavigation,
  goToWorkspacesCreate: unexpectedNavigation,
  goToWorkspace: unexpectedNavigation,
  goToWorkspaceVsCode: unexpectedNavigation,
  goToExport: unexpectedNavigation,
  goToProject: unexpectedNavigation,
  goToProjectWorkflows: unexpectedNavigation,
  goToProjectWorkflowEdit: unexpectedNavigation,
  goToProjectWorkflowRun: unexpectedNavigation,
  goToProjectTask: unexpectedNavigation,
  goToProjectTaskWorkspace: unexpectedNavigation,
  goToProjectTaskWorkspaceCreate: unexpectedNavigation,
  goToProjectWorkspaceCreate: unexpectedNavigation,
};
const actions = {
  findMatchingDiffPath: (text: string) =>
    text === "code.ts" ? "src/code.ts" : null,
  viewFileInChanges: (path: string) => selectedPaths.push(path),
  hasDiffPath: (path: string) => path === "src/code.ts",
};

function Fixture() {
  const [content, setContent] = useState("Initial **answer**");
  const [historyCount, setHistoryCount] = useState(0);
  const [draft, setDraft] = useState("");

  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | undefined;
    window.chatMarkdownFixture = {
      replace: setContent,
      setHistory: setHistoryCount,
      selectedPaths,
      startStream: () => {
        clearInterval(timer);
        let tick = 0;
        timer = setInterval(() => {
          tick += 1;
          setContent(
            `Streamed **reply** ${tick}\n\n${"More content. ".repeat(tick)}`,
          );
          if (tick === 20) clearInterval(timer);
        }, 30);
      },
    };
    return () => clearInterval(timer);
  }, []);

  return (
    <ChangesViewActionsContext.Provider value={actions}>
      <main
        className="new-design bg-primary text-normal p-double"
        style={{ width: 900 }}
      >
        <section
          data-testid="history"
          style={{ maxHeight: 300, overflow: "auto" }}
        >
          {Array.from({ length: historyCount }, (_, index) => (
            <article key={index} data-history-row={index}>
              <AppChatMarkdown
                content={`History ${index}: **retained** \`code.ts\`\n\n${"A completed paragraph. ".repeat(8)}`}
              />
            </article>
          ))}
        </section>
        <section data-testid="reply">
          <AppChatMarkdown
            content={content}
            workspaceId="markdown-workspace"
            sessionId="markdown-session"
          />
        </section>
        <section data-testid="composer" className="border p-base">
          <WYSIWYGEditor value={draft} onChange={setDraft} />
        </section>
        <output data-testid="draft">{draft}</output>
      </main>
    </ChangesViewActionsContext.Provider>
  );
}

const client = new QueryClient({
  defaultOptions: { queries: { retry: false } },
});
const router = createConversationFixtureRouter(<Fixture />);
createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={client}>
    <AppNavigationProvider value={navigation}>
      <ThemeProvider initialTheme={ThemeMode.LIGHT}>
        <RouterProvider router={router} />
      </ThemeProvider>
    </AppNavigationProvider>
  </QueryClientProvider>,
);
