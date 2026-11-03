import { useCallback, useEffect, useMemo, useRef } from 'react';
import { useBlocker } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import {
  ArrowLeftIcon,
  CloudIcon,
  DesktopIcon,
  GearIcon,
  InfoIcon,
} from '@phosphor-icons/react';
import { Button } from '@vibe/ui/components/Button';
import { ConfirmDialog } from '@vibe/ui/components/ConfirmDialog';
import {
  DegradedState,
  EmptyState,
  ErrorState,
  LoadingState,
  OfflineState,
} from '@vibe/ui/components/StateSurface';
import { GeneralSettingsSection } from '@/shared/dialogs/settings/settings/GeneralSettingsSection';
import { GitConnectionsSettings } from '@/shared/dialogs/settings/settings/GitConnectionsSettings';
import { OrganizationsSettingsSection } from '@/shared/dialogs/settings/settings/OrganizationsSettingsSection';
import { RelaySettingsSectionContent } from '@/shared/dialogs/settings/settings/RelaySettingsSection';
import { ProjectSettingsContent } from './ProjectSettingsContent';
import { IntegrationSettings } from './IntegrationSettings';
import { useNavigate } from '@tanstack/react-router';
import { ReposSettingsSection } from '@/shared/dialogs/settings/settings/ReposSettingsSection';
import { SettingsCard } from '@/shared/dialogs/settings/settings/SettingsComponents';
import {
  SettingsHostProvider,
  useSettingsHost,
  useSettingsMachineClient,
} from '@/shared/dialogs/settings/settings/SettingsHostContext';
import { useSettingsDirty } from '@/shared/dialogs/settings/settings/SettingsDirtyContext';
import {
  SettingsMachineUserSystemProvider,
  useSettingsMachineState,
} from '@/shared/dialogs/settings/settings/SettingsMachineUserSystemProvider';
import { useAppRuntime } from '@/shared/hooks/useAppRuntime';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import {
  useAppUpdateStore,
  type AppUpdateMonitoringRetry,
  type AppUpdateMonitoringStatus,
  type AppUpdateRestart,
} from '@/shared/stores/useAppUpdateStore';
import {
  projectAppUpdateSurface,
  type AppUpdateSurfaceProjection,
} from '../model/appUpdate';
import {
  isCanonicalSettingsSearch,
  resolveSettingsRoute,
  SETTINGS_SECTIONS,
  SETTINGS_TABS,
  type ResolvedSettingsRoute,
  type SettingsSearchParams,
  type SettingsSection,
} from '../model/settingsRoute';
import './settings-page.css';

interface SettingsPageProps {
  search: SettingsSearchParams;
  onSearchChange(
    search: ResolvedSettingsRoute,
    options?: { replace?: boolean }
  ): void;
}

const TAB_ICONS = {
  general: GearIcon,
  host: DesktopIcon,
  cloud: CloudIcon,
} as const;

const TAB_LABELS = {
  general: 'Application settings',
  host: 'Current Host',
  cloud: 'Cloud',
} as const;

const SECTION_LABELS = {
  application: 'Application',
  repositories: 'Repositories',
  integrations: 'External integrations',
  relay: 'Remote Access',
  organizations: 'Organizations',
  projects: 'Projects',
} as const;

export function SettingsPage({ search, onSearchChange }: SettingsPageProps) {
  const { t } = useTranslation('settings');
  const navigate = useNavigate();
  const { availableHosts, selectedHostId, setSelectedHostId } =
    useSettingsHost();
  const { clearAll: clearDirty, isDirty } = useSettingsDirty();
  const route = useMemo(() => resolveSettingsRoute(search), [search]);
  const isProjectSettings = route.section === 'projects';
  const confirmationPendingRef = useRef(false);
  const allowNavigationRef = useRef(false);
  const navigationBlocker = useBlocker({
    shouldBlockFn: () => isDirty && !allowNavigationRef.current,
    enableBeforeUnload: false,
    withResolver: true,
  });

  useEffect(() => {
    if (!isCanonicalSettingsSearch(search, route)) {
      onSearchChange(route, { replace: true });
    }
  }, [onSearchChange, route, search]);

  const confirmDiscard = useCallback(async () => {
    if (confirmationPendingRef.current) return false;
    confirmationPendingRef.current = true;
    try {
      const result = await ConfirmDialog.show({
        title: t('settings.page.unsaved.title', 'Unsaved settings'),
        message: t(
          'settings.page.unsaved.message',
          'Discard your unsaved changes before leaving this section?'
        ),
        confirmText: t('settings.page.unsaved.discard', 'Discard changes'),
        cancelText: t('settings.page.unsaved.cancel', 'Keep editing'),
        variant: 'destructive',
      });
      return result === 'confirmed';
    } finally {
      confirmationPendingRef.current = false;
    }
  }, [t]);

  const runAfterDirtyConfirmation = useCallback(
    async (action: () => void, navigation = false) => {
      if (!isDirty) {
        action();
        return;
      }
      if (await confirmDiscard()) {
        if (navigation) allowNavigationRef.current = true;
        clearDirty();
        try {
          action();
        } finally {
          if (navigation) {
            queueMicrotask(() => {
              allowNavigationRef.current = false;
            });
          }
        }
      }
    },
    [clearDirty, confirmDiscard, isDirty]
  );

  useEffect(() => {
    if (!isDirty) return;
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', handleBeforeUnload);
    return () => window.removeEventListener('beforeunload', handleBeforeUnload);
  }, [isDirty]);

  useEffect(() => {
    if (
      navigationBlocker.status !== 'blocked' ||
      confirmationPendingRef.current
    ) {
      return;
    }
    let cancelled = false;
    void confirmDiscard().then((discard) => {
      if (cancelled || navigationBlocker.status !== 'blocked') return;
      if (discard) {
        allowNavigationRef.current = true;
        clearDirty();
        navigationBlocker.proceed();
        queueMicrotask(() => {
          allowNavigationRef.current = false;
        });
      } else {
        navigationBlocker.reset();
      }
    });
    return () => {
      cancelled = true;
    };
  }, [clearDirty, confirmDiscard, navigationBlocker]);

  const navigateTo = (next: ResolvedSettingsRoute) => {
    if (next.tab === route.tab && next.section === route.section) return;
    void runAfterDirtyConfirmation(
      () =>
        onSearchChange({
          ...next,
          ...(route.host ? { host: route.host } : {}),
        }),
      true
    );
  };

  const requiresHost = route.tab !== 'cloud';

  return (
    <section
      className="vk-settings-page"
      data-section={route.section}
      aria-labelledby="settings-title"
    >
      <header className="vk-settings-page__header">
        <div>
          {isProjectSettings && (
            <button
              type="button"
              className="vk-settings-page__back"
              aria-label={t('projects:scoped.back', 'Back to projects')}
              onClick={() => void navigate({ to: '/projects' })}
            >
              <ArrowLeftIcon aria-hidden="true" />
              {t('projects:directory.title', 'Projects')}
            </button>
          )}
          {!isProjectSettings && (
            <p className="vk-settings-page__eyebrow">
              {t('settings.page.eyebrow', 'Application and environment')}
            </p>
          )}
          <h1 id="settings-title">
            {isProjectSettings
              ? t('projects:directory.projectSettings', 'Project settings')
              : t('settings.page.title', 'Settings')}
          </h1>
          <p>
            {isProjectSettings
              ? t(
                  'projects:scoped.description',
                  'Settings for this project only.'
                )
              : t(
                  'settings.page.description',
                  'Manage application preferences, the selected host and cloud resources.'
                )}
          </p>
        </div>
        {(route.tab !== 'cloud' || isProjectSettings) &&
          availableHosts.length > 0 && (
            <label className="vk-settings-page__host-picker">
              <span>{t('settings.page.currentHost', 'Current host')}</span>
              <select
                value={selectedHostId ?? ''}
                onChange={(event) => {
                  const hostId = event.target.value;
                  if (hostId === selectedHostId) return;
                  void runAfterDirtyConfirmation(() => {
                    setSelectedHostId(hostId);
                    onSearchChange({ ...route, host: hostId });
                  }, true);
                }}
              >
                {availableHosts.map((host) => (
                  <option key={host.id} value={host.id}>
                    {host.label}
                    {host.status === 'offline'
                      ? ` · ${t('settings.page.states.offline', 'Offline')}`
                      : ''}
                  </option>
                ))}
              </select>
            </label>
          )}
      </header>

      {!isProjectSettings && (
        <nav
          className="vk-settings-page__tabs"
          aria-label={t('settings.page.tabsLabel', 'Settings categories')}
        >
          {SETTINGS_TABS.map((tab) => {
            const Icon = TAB_ICONS[tab];
            return (
              <button
                key={tab}
                type="button"
                aria-current={route.tab === tab ? 'page' : undefined}
                onClick={() =>
                  navigateTo({ tab, section: SETTINGS_SECTIONS[tab][0] })
                }
              >
                <Icon aria-hidden="true" />
                {t(`settings.page.tabs.${tab}`, TAB_LABELS[tab])}
              </button>
            );
          })}
        </nav>
      )}

      {!isProjectSettings && route.tab !== 'general' && (
        <nav
          className="vk-settings-page__sections"
          aria-label={t('settings.page.sectionsLabel', 'Settings sections')}
        >
          {SETTINGS_SECTIONS[route.tab]
            .filter((section) => section !== 'projects')
            .map((section) => (
              <button
                key={section}
                type="button"
                aria-current={route.section === section ? 'page' : undefined}
                onClick={() => navigateTo({ tab: route.tab, section })}
              >
                {t(
                  `settings.page.sections.${section}`,
                  SECTION_LABELS[section]
                )}
              </button>
            ))}
        </nav>
      )}

      <div className="vk-settings-page__content">
        <SettingsContentBoundary
          key={`${selectedHostId ?? 'unselected'}:${route.section}:${route.projectId ?? ''}`}
          requiresHost={requiresHost}
          section={route.section}
          routeHostId={route.host}
          projectId={route.projectId}
        />
      </div>
    </section>
  );
}

function SettingsContentBoundary({
  requiresHost,
  section,
  routeHostId,
  projectId,
}: {
  requiresHost: boolean;
  section: SettingsSection;
  routeHostId?: string;
  projectId?: string;
}) {
  const { t } = useTranslation('settings');
  const { hostDiscovery, selectedHost, selectedHostId } = useSettingsHost();
  const machineClient = useSettingsMachineClient();
  const machineState = useSettingsMachineState();

  if (!requiresHost) {
    return (
      <SettingsContent
        section={section}
        hostId={routeHostId}
        projectId={projectId}
      />
    );
  }

  if (hostDiscovery.isLoading && !hostDiscovery.hasCanonicalData) {
    return (
      <LoadingState
        className="vk-settings-state"
        title={t(
          'settings.page.states.loadingHosts',
          'Loading available hosts…'
        )}
      />
    );
  }

  if (hostDiscovery.error && !hostDiscovery.hasCanonicalData) {
    return (
      <ErrorState
        className="vk-settings-state"
        title={t(
          'settings.page.states.hostDiscoveryErrorTitle',
          'Hosts could not be loaded'
        )}
        description={t(
          'settings.page.states.hostDiscoveryError',
          'Try loading the available hosts again.'
        )}
        action={
          hostDiscovery.canRetry ? (
            <RetryButton
              label={t('settings.page.states.retryHosts', 'Retry')}
              loadingLabel={t(
                'settings.page.states.retryingHosts',
                'Retrying host discovery'
              )}
              isRetrying={hostDiscovery.isRetrying}
              onRetry={hostDiscovery.retry}
            />
          ) : undefined
        }
      />
    );
  }

  if (!selectedHost || !machineClient) {
    const retryAction = hostDiscovery.canRetry ? (
      <RetryButton
        label={t('settings.page.states.checkHosts', 'Check again')}
        loadingLabel={t(
          'settings.page.states.checkingHosts',
          'Checking available hosts'
        )}
        isRetrying={hostDiscovery.isRetrying}
        onRetry={hostDiscovery.retry}
      />
    ) : undefined;

    if (selectedHostId != null) {
      return (
        <ErrorState
          className="vk-settings-state"
          title={t(
            'settings.page.states.selectedHostUnavailableTitle',
            'Selected host unavailable'
          )}
          description={t(
            'settings.page.states.selectedHostUnavailable',
            'This host is not currently available to this application. Choose another host or try again.'
          )}
          action={retryAction}
        />
      );
    }

    return (
      <EmptyState
        className="vk-settings-state"
        title={t('settings.page.states.noHostTitle', 'No host available')}
        description={t(
          'settings.page.states.noHost',
          'Connect a host before managing host-owned settings.'
        )}
        action={retryAction}
      />
    );
  }

  if (selectedHost.kind === 'remote' && selectedHost.status === 'offline') {
    return (
      <OfflineState
        className="vk-settings-state"
        title={t('settings.page.states.hostOfflineTitle', 'Host unavailable')}
        description={t(
          'settings.page.states.hostOffline',
          'Reconnect this host before viewing or changing its settings.'
        )}
        action={
          hostDiscovery.canRetry ? (
            <RetryButton
              label={t('settings.page.states.checkHosts', 'Check again')}
              loadingLabel={t(
                'settings.page.states.checkingHosts',
                'Checking available hosts'
              )}
              isRetrying={hostDiscovery.isRetrying}
              onRetry={hostDiscovery.retry}
            />
          ) : undefined
        }
      />
    );
  }

  if (machineState.isLoading && !machineState.hasCanonicalData) {
    return (
      <LoadingState
        className="vk-settings-state"
        title={t(
          'settings.page.states.loadingHostSettings',
          'Loading host settings…'
        )}
      />
    );
  }

  if (machineState.error && !machineState.hasCanonicalData) {
    return (
      <ErrorState
        className="vk-settings-state"
        title={t(
          'settings.page.states.hostSettingsErrorTitle',
          'Host settings could not be loaded'
        )}
        description={t(
          'settings.page.states.hostSettingsError',
          'The selected host is available, but its settings request failed.'
        )}
        action={
          <RetryButton
            label={t('settings.page.states.retrySettings', 'Retry')}
            loadingLabel={t(
              'settings.page.states.retryingSettings',
              'Retrying host settings'
            )}
            isRetrying={machineState.isRetrying}
            onRetry={machineState.retry}
          />
        }
      />
    );
  }

  const hostDiscoveryDegraded =
    hostDiscovery.hasCanonicalData && hostDiscovery.error != null;
  const machineSettingsDegraded =
    machineState.hasCanonicalData && machineState.error != null;
  const isDegraded = hostDiscoveryDegraded || machineSettingsDegraded;
  const isRetrying =
    (hostDiscoveryDegraded && hostDiscovery.isRetrying) ||
    (machineSettingsDegraded && machineState.isRetrying);
  const retryDegraded = async () => {
    await Promise.all([
      hostDiscoveryDegraded ? hostDiscovery.retry() : Promise.resolve(),
      machineSettingsDegraded ? machineState.retry() : Promise.resolve(),
    ]);
  };

  return (
    <>
      {isDegraded && (
        <DegradedState
          id="settings-degraded-state"
          compact
          className="vk-settings-page__degraded"
          title={t(
            'settings.page.states.degradedTitle',
            'Settings are temporarily read-only'
          )}
          description={t(
            'settings.page.states.degraded',
            'Previously loaded settings remain visible, but changes are disabled until the refresh succeeds.'
          )}
          action={
            <RetryButton
              label={t('settings.page.states.retryRefresh', 'Retry refresh')}
              loadingLabel={t(
                'settings.page.states.retryingRefresh',
                'Retrying settings refresh'
              )}
              isRetrying={isRetrying}
              onRetry={retryDegraded}
            />
          }
        />
      )}
      <fieldset
        className="vk-settings-page__host-content"
        disabled={isDegraded}
        aria-describedby={isDegraded ? 'settings-degraded-state' : undefined}
      >
        <SettingsContent section={section} hostId={routeHostId} />
      </fieldset>
    </>
  );
}

function RetryButton({
  label,
  loadingLabel,
  isRetrying,
  onRetry,
}: {
  label: string;
  loadingLabel: string;
  isRetrying: boolean;
  onRetry: () => Promise<void>;
}) {
  return (
    <Button
      className="min-h-11"
      type="button"
      loading={isRetrying}
      loadingLabel={loadingLabel}
      onClick={() => void onRetry()}
    >
      {label}
    </Button>
  );
}

function SettingsContent({
  section,
  hostId,
  projectId,
}: {
  section: SettingsSection;
  hostId?: string;
  projectId?: string;
}) {
  switch (section) {
    case 'application':
      return (
        <>
          <GeneralSettingsSection includeAgentSettings={false} />
          <VersionSettings />
          <UnavailableSettings />
        </>
      );
    case 'repositories':
      return (
        <>
          <GitConnectionsSettings />
          <ReposSettingsSection />
        </>
      );
    case 'integrations':
      return <IntegrationSettings />;
    case 'relay':
      return <RelaySettingsContent hostId={hostId} />;
    case 'organizations':
      return <OrganizationsSettingsSection />;
    case 'projects':
      return projectId ? (
        <ProjectSettingsContent projectId={projectId} />
      ) : (
        <ProjectSettingsSelectionHint />
      );
  }
}

function ProjectSettingsSelectionHint() {
  const { t } = useTranslation('projects');
  return (
    <p>
      {t(
        'scoped.noSelection',
        'Open a project’s menu in Projects to edit its settings.'
      )}
    </p>
  );
}

function VersionSettings() {
  const { t } = useTranslation('settings');
  const runtime = useAppRuntime();
  const { selectedHost } = useSettingsHost();
  const { appVersion } = useUserSystem();
  const snapshot = useAppUpdateStore((state) => state.snapshot);
  const monitoringStatus = useAppUpdateStore((state) => state.monitoringStatus);
  const monitoringRetry = useAppUpdateStore((state) => state.monitoringRetry);
  const restartStatus = useAppUpdateStore((state) => state.restartStatus);
  const requestRestart = useAppUpdateStore((state) => state.requestRestart);
  const update = projectAppUpdateSurface({
    runtime,
    hostKind: selectedHost?.kind ?? null,
    snapshot,
    monitoringStatus,
    restartStatus,
  });

  return (
    <SettingsCard
      title={t('settings.page.version.title', 'Version and updates')}
      description={t(
        'settings.page.version.description',
        'Review the version reported by this host and update facts reported by the desktop runtime.'
      )}
    >
      <div className="vk-settings-page__version-row">
        <div>
          <span>{t('settings.page.version.current', 'Current version')}</span>
          <strong>
            {appVersion || t('settings.page.version.unknown', 'Unavailable')}
          </strong>
        </div>
        <AppUpdateStatus
          update={update}
          monitoringRetry={monitoringRetry}
          monitoringStatus={monitoringStatus}
          onRestart={requestRestart}
        />
      </div>
    </SettingsCard>
  );
}

function AppUpdateStatus({
  update,
  monitoringRetry,
  monitoringStatus,
  onRestart,
}: {
  update: AppUpdateSurfaceProjection;
  monitoringRetry: AppUpdateMonitoringRetry | null;
  monitoringStatus: AppUpdateMonitoringStatus;
  onRestart: AppUpdateRestart;
}) {
  const { t } = useTranslation('settings');

  const retryMonitoring = monitoringRetry ? (
    <Button
      className="min-h-11"
      type="button"
      loading={monitoringStatus === 'connecting'}
      loadingLabel={t(
        'settings.page.version.reconnecting',
        'Reconnecting update status'
      )}
      onClick={() => void monitoringRetry()}
    >
      {t('settings.page.version.retryMonitoring', 'Retry')}
    </Button>
  ) : undefined;

  if (update.kind === 'unavailable') {
    const description = {
      'remote-runtime': t(
        'settings.page.version.remoteRuntime',
        'Desktop updates are managed by the installed application on that machine, not by the Remote web interface.'
      ),
      'remote-host': t(
        'settings.page.version.remoteHost',
        'This application cannot update the selected remote host.'
      ),
      'host-unavailable': t(
        'settings.page.version.hostUnavailable',
        'Select this local host to view its desktop update status.'
      ),
      browser: t(
        'settings.page.version.browserRuntime',
        'Automatic application updates are available only in the installed desktop application.'
      ),
    }[update.reason];

    return (
      <div
        className="vk-settings-page__update-status"
        data-update-phase="unavailable"
      >
        <span>
          {t('settings.page.version.updateStatus', 'Application update')}
        </span>
        <strong>{t('settings.page.version.unavailable', 'Unavailable')}</strong>
        <p>{description}</p>
      </div>
    );
  }

  if (update.kind === 'connecting') {
    return (
      <LoadingState
        compact
        className="vk-settings-page__update-state"
        title={t(
          'settings.page.version.connecting',
          'Connecting to desktop update status…'
        )}
        description={t(
          'settings.page.version.connectingDescription',
          'This connects to update events; it does not mean an update check is running.'
        )}
      />
    );
  }

  if (update.kind === 'monitoring-error') {
    return (
      <ErrorState
        compact
        className="vk-settings-page__update-state"
        title={t(
          'settings.page.version.monitoringErrorTitle',
          'Update status could not be connected'
        )}
        description={t(
          'settings.page.version.monitoringError',
          'The application could not subscribe to desktop update events.'
        )}
        action={retryMonitoring}
      />
    );
  }

  if (update.kind === 'initial') {
    return (
      <div
        className="vk-settings-page__update-status"
        data-update-phase="initial"
      >
        <span>
          {t('settings.page.version.updateStatus', 'Application update')}
        </span>
        <strong>
          {t('settings.page.version.automatic', 'Automatic updates active')}
        </strong>
        <p>
          {t(
            'settings.page.version.initial',
            'The desktop runtime checks and downloads updates in the background. It has not reported an available update in this app session.'
          )}
        </p>
      </div>
    );
  }

  return (
    <div className="vk-settings-page__update-stack">
      {update.degraded && (
        <DegradedState
          compact
          className="vk-settings-page__update-state"
          title={t(
            'settings.page.version.degradedTitle',
            'Update information may be stale'
          )}
          description={t(
            'settings.page.version.degraded',
            'The last reported update remains visible, but the desktop event connection needs to be restored.'
          )}
          action={retryMonitoring}
        />
      )}
      <div
        className="vk-settings-page__update-status"
        data-update-phase={update.kind}
        role="status"
        aria-live="polite"
      >
        <span>
          {t('settings.page.version.updateStatus', 'Application update')}
        </span>
        <strong>
          {update.kind === 'restart-ready'
            ? t('settings.page.version.update', 'Downloaded update')
            : t('settings.page.version.reported', 'Update reported')}
        </strong>
        <p>
          {update.kind === 'restart-ready'
            ? t(
                'settings.page.version.ready',
                'Version {{version}} is downloaded and ready to install.',
                { version: update.version }
              )
            : t(
                'settings.page.version.available',
                'The desktop runtime reported version {{version}}. It will notify the application when the download is ready.',
                { version: update.version }
              )}
        </p>
        {update.releaseNotes && (
          <details className="vk-settings-page__update-notes">
            <summary>
              {t('settings.page.version.releaseNotes', "What's new")}
            </summary>
            <p>{update.releaseNotes}</p>
          </details>
        )}
        {update.kind === 'restart-ready' && (
          <div className="vk-settings-page__update-action">
            {update.restartStatus === 'error' && (
              <p role="alert">
                {t(
                  'settings.page.version.restartError',
                  'The restart request could not be sent. The downloaded update remains ready.'
                )}
              </p>
            )}
            <Button
              className="min-h-11"
              type="button"
              loading={update.restartStatus === 'pending'}
              loadingLabel={t(
                'settings.page.version.restarting',
                'Requesting application restart'
              )}
              onClick={() => void onRestart()}
            >
              {update.restartStatus === 'error'
                ? t('settings.page.version.retryRestart', 'Retry restart')
                : t('settings.page.version.restart', 'Restart and install')}
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}

function RelaySettingsContent({ hostId }: { hostId?: string }) {
  const runtime = useAppRuntime();

  if (runtime !== 'local') {
    return <RelaySettingsSectionContent initialState={{ hostId }} />;
  }

  // Local Relay APIs always address this process. Isolate the section from
  // any remote host selected on another tab so config and mutations cannot
  // cross host ownership boundaries.
  return (
    <SettingsHostProvider initialHostId="local">
      <SettingsMachineUserSystemProvider>
        <RelaySettingsSectionContent />
      </SettingsMachineUserSystemProvider>
    </SettingsHostProvider>
  );
}

function UnavailableSettings() {
  const { t } = useTranslation('settings');
  const unavailable = [
    ['density', 'Density'],
    ['fontScale', 'Desktop font scale'],
    ['updateChannel', 'Update channel'],
    ['backupRestore', 'Backup and restore'],
    ['diagnostics', 'Diagnostics'],
    ['notifications', 'System notification management'],
    ['dataDirectory', 'Data directory management'],
  ] as const;

  return (
    <SettingsCard
      title={t('settings.page.unavailable.title', 'Planned settings')}
      description={t(
        'settings.page.unavailable.description',
        'These settings are visible for clarity but are not configurable yet.'
      )}
    >
      <div className="vk-settings-page__unavailable-grid">
        {unavailable.map(([key, label]) => (
          <div key={key} className="vk-settings-page__unavailable-item">
            <InfoIcon aria-hidden="true" />
            <span>{t(`settings.page.unavailable.${key}`, label)}</span>
            <small>
              {t('settings.page.unavailable.status', 'Unavailable')}
            </small>
          </div>
        ))}
      </div>
    </SettingsCard>
  );
}
