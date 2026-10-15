import * as React from 'react';
import { createRoot } from 'react-dom/client';
import { HotkeysProvider } from '../../../../packages/ui/node_modules/react-hotkeys-hook';
import { Dialog as KeyboardDialog } from '../../../../packages/ui/src/components/KeyboardDialog';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import {
  RouterContextProvider,
  createRootRoute,
  createRouter,
} from '../../../../packages/web-core/node_modules/@tanstack/react-router';
import {
  FloatingPanel,
  FloatingPanelBody,
  FloatingPanelDescription,
  FloatingPanelHeader,
  FloatingPanelTitle,
} from '../../../../packages/ui/src/components/FloatingPanel';
import { Button } from '../../../../packages/ui/src/components/Button';
import { ConfirmDialogView } from '../../../../packages/ui/src/components/ConfirmDialog';
import { Input } from '../../../../packages/ui/src/components/Input';
import { SplitLayout } from '../../../../packages/ui/src/components/SplitLayout';
import { CrashScreen } from '../../../../packages/ui/src/components/CrashScreen';
import { StateSurface } from '../../../../packages/ui/src/components/StateSurface';
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '../../../../packages/ui/src/components/Dialog';
import { ExportChooseProjects } from '../../../../packages/web-core/src/features/export/ui/ExportChooseProjects';
import { ProjectSunsetPage } from '../../../../packages/web-core/src/pages/kanban/ProjectSunsetPage';
import { NotificationsPage } from '../../../../packages/web-core/src/pages/workspaces/NotificationsPage';
import {
  AuthContext,
  type AuthContextValue,
} from '../../../../packages/web-core/src/shared/hooks/auth/useAuth';
import { configureAuthRuntime } from '../../../../packages/web-core/src/shared/lib/auth/runtime';
import { setLocalRemoteApiEnabled } from '../../../../packages/web-core/src/shared/lib/remoteApi';
import { SimpleMarkdown } from '../../../../packages/web-core/src/shared/components/SimpleMarkdown';
import {
  AppNavigationProvider,
  type AppNavigation,
} from '../../../../packages/web-core/src/shared/hooks/useAppNavigation';
import {
  createBrowserThemeController,
  type ThemeMode,
} from '../../../../packages/ui/src/lib/theme';
import '../../../../packages/ui/src/styles/tokens.css';
import './style.css';

if (new URLSearchParams(window.location.search).has('confirmation')) {
  await import('./tailwind.css');
  // Match the application: generated base/utilities precede component CSS.
  const utilities = document.querySelector(
    'style[data-vite-dev-id$="/tailwind.css"]'
  );
  if (utilities) document.head.prepend(utilities);
}

const root = document.documentElement;
const bootstrapSnapshot = {
  mode: root.dataset.themeMode,
  theme: root.dataset.theme,
  colorScheme: root.style.colorScheme,
};

const createdController = createBrowserThemeController();

if (!createdController) {
  throw new Error('Theme controller was not created in the browser fixture');
}

const controller = createdController;

// Keep browser evidence on the production NotificationsPage boundaries while
// using local fallback requests that the Playwright suite can mock.
configureAuthRuntime({
  getToken: async () => 'ui-foundation-fixture-token',
  triggerRefresh: async () => 'ui-foundation-fixture-token',
  registerShape: () => () => undefined,
  getCurrentUser: async () => ({ user_id: 'fixture-user' }),
});
setLocalRemoteApiEnabled(true);

const fixtureAuth: AuthContextValue = {
  isSignedIn: true,
  isLoaded: true,
  userId: 'fixture-user',
};

const notificationsRootRoute = createRootRoute({ component: () => null });
const notificationsRouter = createRouter({
  routeTree: notificationsRootRoute,
});
const notificationsQueryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
    },
  },
});

function getRequiredElement(testId: string): HTMLElement {
  const element = document.querySelector<HTMLElement>(
    `[data-testid="${testId}"]`
  );
  if (!element) {
    throw new Error(`Theme fixture output is missing: ${testId}`);
  }
  return element;
}

const bootstrapOutput = getRequiredElement('bootstrap-snapshot');
const modeOutput = getRequiredElement('controller-mode');
const effectiveThemeOutput = getRequiredElement('effective-theme');
const mountCountOutput = getRequiredElement('mount-count');

bootstrapOutput.textContent = JSON.stringify(bootstrapSnapshot);

const mountCount = Number(root.dataset.fixtureMountCount ?? '0') + 1;
root.dataset.fixtureMountCount = String(mountCount);
mountCountOutput.textContent = String(mountCount);

if (window.location.pathname === '/') {
  window.history.replaceState(
    { fixture: 'theme-foundations' },
    '',
    '/workspace/session-42?fixture=theme#transcript'
  );
}

function renderControllerState() {
  modeOutput.textContent = controller.getMode();
  effectiveThemeOutput.textContent = controller.getEffectiveTheme();
}

document
  .querySelectorAll<HTMLButtonElement>('[data-theme-mode]')
  .forEach((button) => {
    button.addEventListener('click', () => {
      controller.setMode(button.dataset.themeMode as ThemeMode);
      renderControllerState();
    });
  });

renderControllerState();

function FloatingPanelHarness() {
  const [open, setOpen] = React.useState(false);
  const [autoFocus, setAutoFocus] = React.useState(false);

  const openPanel = (shouldAutoFocus: boolean) => {
    setAutoFocus(shouldAutoFocus);
    setOpen(true);
  };

  return (
    <section aria-labelledby="floating-panel-contract-title">
      <h2 id="floating-panel-contract-title">Floating panel contract</h2>
      <div className="fixture-actions">
        <button
          type="button"
          data-testid="panel-trigger"
          onClick={() => openPanel(false)}
        >
          Open default panel
        </button>
        <button
          type="button"
          data-testid="canvas-target"
          onClick={() => openPanel(false)}
        >
          Canvas target
        </button>
        <button
          type="button"
          data-testid="autofocus-trigger"
          onClick={() => openPanel(true)}
        >
          Open autofocus panel
        </button>
        <button
          type="button"
          data-testid="canvas-close-panel"
          onClick={() => setOpen(false)}
        >
          Focus canvas and close panel
        </button>
      </div>

      <FloatingPanel
        open={open}
        onOpenChange={setOpen}
        autoFocus={autoFocus}
        aria-labelledby="fixture-panel-title"
        aria-describedby="fixture-panel-description"
        contentClassName="fixture-floating-panel-content"
      >
        <FloatingPanelHeader>
          <FloatingPanelTitle id="fixture-panel-title">
            Node configuration
          </FloatingPanelTitle>
          <FloatingPanelDescription id="fixture-panel-description">
            Non-modal configuration that keeps the canvas available.
          </FloatingPanelDescription>
        </FloatingPanelHeader>
        <FloatingPanelBody>
          <label>
            Node title
            <input data-testid="panel-input" defaultValue="Review changes" />
          </label>
        </FloatingPanelBody>
      </FloatingPanel>
    </section>
  );
}

function SplitLayoutHarness() {
  const [secondarySize, setSecondarySize] = React.useState(320);
  const [resizeEndSize, setResizeEndSize] = React.useState<number | null>(null);

  return (
    <section aria-labelledby="split-layout-contract-title">
      <h2 id="split-layout-contract-title">Split layout contract</h2>
      <output data-testid="secondary-size">{secondarySize}</output>
      <output data-testid="resize-end-size">{resizeEndSize ?? 'none'}</output>
      <SplitLayout
        data-testid="split-layout"
        primary={<div>Primary work area</div>}
        secondary={<div>Secondary inspector</div>}
        secondarySize={secondarySize}
        onSecondarySizeChange={setSecondarySize}
        minSecondarySize={240}
        maxSecondarySize={400}
        resizeStep={40}
        separatorLabel="Resize test inspector"
        secondaryId="fixture-secondary-pane"
        onResizeEnd={setResizeEndSize}
      />
    </section>
  );
}

function InteractionContractHarness() {
  const [childCaptureCount, setChildCaptureCount] = React.useState(0);
  const [childClickCount, setChildClickCount] = React.useState(0);
  const [buttonCaptureCount, setButtonCaptureCount] = React.useState(0);

  return (
    <section aria-labelledby="interaction-contract-title">
      <h2 id="interaction-contract-title">Interaction contracts</h2>
      <Button
        asChild
        disabled
        onClickCapture={() => setButtonCaptureCount((count) => count + 1)}
      >
        <a
          href="#disabled-button-should-not-navigate"
          data-testid="disabled-as-child"
          onClickCapture={() => setChildCaptureCount((count) => count + 1)}
          onClick={() => setChildClickCount((count) => count + 1)}
        >
          Disabled slotted action
        </a>
      </Button>
      <Button
        type="button"
        data-testid="idle-loading-label"
        loadingLabel="Saving"
      >
        Save
      </Button>
      <Button
        type="button"
        data-testid="active-loading-label"
        loading
        loadingLabel="Saving"
      >
        Save
      </Button>
      <output data-testid="child-capture-count">{childCaptureCount}</output>
      <output data-testid="child-click-count">{childClickCount}</output>
      <output data-testid="button-capture-count">{buttonCaptureCount}</output>

      <label htmlFor="invalid-agent-endpoint">Agent endpoint</label>
      <Input
        id="invalid-agent-endpoint"
        data-testid="invalid-input"
        invalid
        aria-describedby="invalid-agent-endpoint-error"
        defaultValue="not-a-valid-endpoint"
      />
      <p id="invalid-agent-endpoint-error">Enter a valid endpoint.</p>

      <div className="fixture-motion-probe" data-testid="motion-probe" />
    </section>
  );
}

function CrashScreenHarness() {
  const [reloadCount, setReloadCount] = React.useState(0);

  return (
    <section aria-labelledby="crash-screen-contract-title">
      <h2 id="crash-screen-contract-title">Crash screen contract</h2>
      <output data-testid="crash-reload-count">{reloadCount}</output>
      <div data-testid="crash-screen-harness">
        <CrashScreen
          error="Fixture crash"
          componentStack="\n at Fixture"
          onReload={() => setReloadCount((count) => count + 1)}
        />
      </div>
    </section>
  );
}

function ExportHarness() {
  const [selectedOrgId, setSelectedOrgId] = React.useState('org-1');
  const [submitted, setSubmitted] = React.useState<string[]>([]);

  return (
    <section aria-labelledby="export-contract-title">
      <h2 id="export-contract-title">Export contract</h2>
      <ExportChooseProjects
        organizations={[
          { id: 'org-1', name: 'Primary organization' },
          { id: 'org-2', name: 'Secondary organization' },
        ]}
        orgsLoading={false}
        orgsError={null}
        onRetryOrganizations={() => undefined}
        projects={[
          { id: 'project-1', name: 'Long project name for narrow screens' },
          { id: 'project-2', name: 'Second project' },
        ]}
        projectsLoading={false}
        projectsError={null}
        onRetryProjects={() => undefined}
        selectedOrgId={selectedOrgId}
        onOrgChange={setSelectedOrgId}
        onContinue={(_orgId, projectIds) => setSubmitted(projectIds)}
      />
      <output data-testid="export-submitted">
        {submitted.length > 0 ? submitted.join(',') : 'none'}
      </output>
    </section>
  );
}

function ProjectSunsetHarness() {
  const [destination, setDestination] = React.useState('none');
  const navigation = React.useMemo<AppNavigation>(
    () =>
      ({
        resolveFromPath: () => null,
        goToRoot: () => undefined,
        goToOnboarding: () => undefined,
        goToOnboardingSignIn: () => undefined,
        goToWorkspaces: () => undefined,
        goToWorkspacesCreate: () => undefined,
        goToWorkspace: () => undefined,
        goToWorkspaceVsCode: () => undefined,
        goToExport: () => setDestination('export'),
        goToProject: () => undefined,
        goToProjectWorkflows: () => undefined,
        goToProjectWorkflowEdit: () => undefined,
        goToProjectWorkflowRun: () => undefined,
        goToProjectTask: () => undefined,
        goToProjectTaskWorkspace: () => undefined,
        goToProjectTaskWorkspaceCreate: () => undefined,
        goToProjectWorkspaceCreate: () => undefined,
      }) as AppNavigation,
    []
  );

  return (
    <section aria-labelledby="project-sunset-contract-title">
      <h2 id="project-sunset-contract-title">Project sunset contract</h2>
      <div className="fixture-sunset">
        <AppNavigationProvider value={navigation}>
          <ProjectSunsetPage projectName="Long project name for narrow screens" />
        </AppNavigationProvider>
      </div>
      <output data-testid="sunset-destination">{destination}</output>
    </section>
  );
}

function NotificationsHarness() {
  return (
    <section aria-labelledby="notifications-contract-title">
      <h2 id="notifications-contract-title">Notifications contract</h2>
      <div className="fixture-notifications">
        <QueryClientProvider client={notificationsQueryClient}>
          <RouterContextProvider router={notificationsRouter}>
            <AuthContext.Provider value={fixtureAuth}>
              <NotificationsPage />
            </AuthContext.Provider>
          </RouterContextProvider>
        </QueryClientProvider>
      </div>
    </section>
  );
}

function NotFoundHarness() {
  const [destination, setDestination] = React.useState('none');

  return (
    <section aria-labelledby="not-found-contract-title">
      <h2 id="not-found-contract-title">404 page contract</h2>
      <div className="fixture-not-found">
        <StateSurface
          state="empty"
          title={<h1>Page not found</h1>}
          description="The page you requested does not exist or is no longer available."
          action={
            <Button
              className="min-h-11"
              size="lg"
              variant="secondary"
              onClick={() => setDestination('home')}
            >
              Back to home
            </Button>
          }
        />
      </div>
      <output data-testid="not-found-destination">{destination}</output>
    </section>
  );
}

const releaseNotesBody = `## Highlights

* Improved workflow navigation and keyboard recovery.
* Long content remains readable on narrow screens.
${Array.from(
  { length: 48 },
  (_, index) => `* Follow-up detail ${index + 1}: keyboard and touch behavior.`
).join('\n')}

Read the [full release](https://github.com/toby1123yjh/vibe-studio/releases).`;

function KeyboardMotionHarness() {
  const [open, setOpen] = React.useState(false);
  return (
    <HotkeysProvider initiallyActiveScopes={['kanban', 'projects']}>
      <button data-testid="keyboard-motion-open" onClick={() => setOpen(true)}>
        Open keyboard dialog
      </button>
      <KeyboardDialog
        open={open}
        onOpenChange={setOpen}
        data-testid="keyboard-motion-dialog"
      >
        <button
          data-testid="keyboard-motion-close"
          onClick={() => setOpen(false)}
        >
          Dismiss
        </button>
      </KeyboardDialog>
    </HotkeysProvider>
  );
}

function ReleaseNotesHarness() {
  const [open, setOpen] = React.useState(false);

  return (
    <section aria-labelledby="release-notes-contract-title">
      <h2 id="release-notes-contract-title">Release notes contract</h2>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogTrigger asChild>
          <button type="button" data-testid="release-notes-trigger">
            Open release notes
          </button>
        </DialogTrigger>
        <DialogContent
          className="fixture-release-notes-content"
          aria-describedby="fixture-release-notes-description"
        >
          <DialogHeader className="fixture-release-notes-header">
            <DialogTitle>What&apos;s New</DialogTitle>
            <DialogDescription id="fixture-release-notes-description">
              Release notes for recent Vibe Studio versions.
            </DialogDescription>
          </DialogHeader>
          <div className="fixture-release-notes-body">
            <article>
              <div className="fixture-release-notes-meta">
                <h3>0.1.44</h3>
                <span>Sep 3, 2026</span>
              </div>
              <SimpleMarkdown content={releaseNotesBody} />
            </article>
            <article>
              <div className="fixture-release-notes-meta">
                <h3>0.1.43</h3>
                <span>Aug 28, 2026</span>
              </div>
              <SimpleMarkdown content="* Stability updates for local agent sessions." />
            </article>
          </div>
          <DialogFooter className="fixture-release-notes-footer">
            <DialogClose asChild>
              <button type="button" data-testid="release-notes-close">
                Close
              </button>
            </DialogClose>
            <button type="button">Open on GitHub</button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}

function ConfirmationHarness() {
  const [mode, setMode] = React.useState<'confirm' | 'single' | null>(null);
  const [pending, setPending] = React.useState(false);
  const [result, setResult] = React.useState('none');
  const close = (value: string) => {
    setResult(value);
    setPending(false);
    setMode(null);
  };
  return (
    <section aria-label="Confirmation contract">
      <button
        data-testid="confirmation-open"
        onClick={() => setMode('confirm')}
      >
        Open confirmation
      </button>
      <button
        data-testid="confirmation-single"
        onClick={() => setMode('single')}
      >
        Open acknowledgement
      </button>
      <output data-testid="confirmation-result">{result}</output>
      <ConfirmDialogView
        open={mode !== null}
        title="确认删除这个会话？"
        message={
          '删除会话和历史记录，其他项目不受影响。'.repeat(8) +
          '\nF:\\workspaces\\' +
          'very-long-directory-name-'.repeat(12)
        }
        variant={mode === 'confirm' ? 'destructive' : 'info'}
        confirmText={pending ? '处理中' : '确定'}
        cancelText="取消"
        showCancelButton={mode !== 'single'}
        confirmPending={pending}
        cancelDisabled={pending}
        onConfirm={() =>
          mode === 'single' ? close('confirmed') : setPending(true)
        }
        onCancel={() => close('canceled')}
      />
    </section>
  );
}

const componentRoot = document.getElementById('component-root');
if (!componentRoot) {
  throw new Error('Component contract root is missing');
}

createRoot(componentRoot).render(
  <React.StrictMode>
    <FloatingPanelHarness />
    <SplitLayoutHarness />
    <InteractionContractHarness />
    <CrashScreenHarness />
    <ExportHarness />
    <ProjectSunsetHarness />
    <NotificationsHarness />
    <NotFoundHarness />
    <ReleaseNotesHarness />
    <KeyboardMotionHarness />
    <ConfirmationHarness />
  </React.StrictMode>
);
