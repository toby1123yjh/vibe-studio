import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import { ProjectKanbanContainer } from "@/features/projects/ui/ProjectKanbanContainer";
import i18n from "@/i18n/config";
import "../../../../packages/ui/src/styles/tokens.css";
import "./style.css";

void i18n.changeLanguage(
  new URLSearchParams(location.search).get("locale") ?? "en",
);
const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false } },
});
const router = createRouter({
  routeTree: createRootRoute({
    component: () => <ProjectKanbanContainer projectName="Activity project" />,
  }),
  history: createMemoryHistory({ initialEntries: ["/"] }),
});

createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={queryClient}>
    <RouterProvider router={router} />
  </QueryClientProvider>,
);
