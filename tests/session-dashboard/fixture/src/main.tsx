import { useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { ThemeMode } from 'shared/types';
import i18n from '@/i18n/config';
import { DashboardPage } from '@/features/app-shell/ui/DashboardPage';
import { PageCanvas } from '@/features/app-shell/ui/PageCanvas';
import { DashboardScopeProvider } from '@/features/session-dashboard/model/DashboardScope';
import { ThemeProvider } from '@/shared/providers/ThemeProvider';
import { useTheme } from '@/shared/hooks/useTheme';
import { AppNavigationProvider } from '@/shared/hooks/useAppNavigation';
import type { AppNavigation } from '@/shared/lib/routes/appNavigation';
import { createConversationFixtureRouter, RouterProvider } from '../../../workflow/fixture/src/conversation-support';
import '@/app/styles/new/index.css';
import '../../../../packages/ui/src/styles/tokens.css';
import '@/features/app-shell/ui/app-shell.css';
import './style.css';

void i18n.changeLanguage(new URLSearchParams(location.search).get('locale') ?? 'en');

const unexpectedNavigation = () => { throw new Error('Unexpected chat navigation'); };
const navigation: AppNavigation = {
  resolveFromPath: () => null,
  goToRoot: unexpectedNavigation, goToOnboarding: unexpectedNavigation,
  goToOnboardingSignIn: unexpectedNavigation, goToWorkspaces: unexpectedNavigation,
  goToWorkspacesCreate: unexpectedNavigation, goToWorkspace: unexpectedNavigation,
  goToWorkspaceVsCode: unexpectedNavigation, goToExport: unexpectedNavigation,
  goToProject: unexpectedNavigation, goToProjectWorkflows: unexpectedNavigation,
  goToProjectWorkflowEdit: unexpectedNavigation, goToProjectWorkflowRun: unexpectedNavigation,
  goToProjectTask: unexpectedNavigation, goToProjectTaskWorkspace: unexpectedNavigation,
  goToProjectTaskWorkspaceCreate: unexpectedNavigation, goToProjectWorkspaceCreate: unexpectedNavigation,
};

function Fixture() {
  const [hostId, setHostId] = useState<string | null>(null);
  const [user, setUser] = useState('user-1');
  const [destination, setDestination] = useState('');
  const { setTheme } = useTheme();
  const scope = useMemo(() => ({
    hostId, scopeKey: JSON.stringify(['dashboard-fixture', hostId, user]),
    deployment: 'local' as const, navigateToRoute: setDestination,
  }), [hostId, user]);
  return <div className="dashboard-fixture">
    <nav aria-label="Fixture controls">
      <button onClick={() => setHostId('host-a')}>Host A</button>
      <button onClick={() => setHostId('host-b')}>Host B</button>
      <button onClick={() => setUser('user-2')}>Other user</button>
      <button onClick={() => setTheme(ThemeMode.DARK)}>Dark</button>
      <button onClick={() => setTheme(ThemeMode.LIGHT)}>Light</button>
      <output data-testid="destination">{destination}</output>
    </nav>
    <PageCanvas mode="full-bleed"><DashboardScopeProvider value={scope}><DashboardPage /></DashboardScopeProvider></PageCanvas>
  </div>;
}

const router = createConversationFixtureRouter(<Fixture />);
createRoot(document.getElementById('root')!).render(
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <AppNavigationProvider value={navigation}><ThemeProvider initialTheme={ThemeMode.LIGHT}><RouterProvider router={router} /></ThemeProvider></AppNavigationProvider>
  </QueryClientProvider>,
);
