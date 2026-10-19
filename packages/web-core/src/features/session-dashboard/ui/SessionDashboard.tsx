import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
} from 'react';
import { RefreshCw, AlertTriangle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Button } from '@vibe/ui/components/Button';
import { ErrorState, LoadingState } from '@vibe/ui/components/StateSurface';
import type { DashboardOpenTarget, DashboardWorkRow } from 'shared/types';
import {
  HostIdScopeProvider,
  useHostId,
} from '@/shared/providers/HostIdProvider';
import { useIsMobile } from '@/shared/hooks/useIsMobile';
import { useDashboard } from '../hooks/useDashboard';
import {
  useDashboardScope,
  type DashboardScope,
} from '../model/DashboardScope';
import { dashboardTargetHref, dashboardWorkKey } from '../model/dashboard';
import { DashboardToolbar } from './DashboardToolbar';
import {
  DashboardTable,
  DashboardTime,
  Pagination,
  type DashboardConversationSelection,
} from './DashboardTable';
import './dashboard.css';

const DashboardConversationPanel = lazy(() =>
  import('./DashboardConversationPanel').then((module) => ({
    default: module.DashboardConversationPanel,
  }))
);

export function SessionDashboard() {
  const shellScope = useDashboardScope();
  const routeHostId = useHostId();
  const fallbackScope = useMemo<DashboardScope>(
    () => ({
      hostId: routeHostId,
      scopeKey: JSON.stringify(['local', routeHostId, 'dashboard']),
      deployment: 'local',
      navigateToRoute: (href) => window.location.assign(href),
    }),
    [routeHostId]
  );
  const scope = shellScope ?? fallbackScope;
  return (
    <HostIdScopeProvider hostId={scope.hostId}>
      <DashboardForScope key={scope.scopeKey} scope={scope} />
    </HostIdScopeProvider>
  );
}

function DashboardForScope({ scope }: { scope: DashboardScope }) {
  const { t } = useTranslation('common');
  const titleId = useId();
  const isMobile = useIsMobile();
  const hostAvailable = scope.deployment !== 'remote' || scope.hostId !== null;
  const dashboard = useDashboard(scope.scopeKey, scope.hostId, hostAvailable);
  const [now, setNow] = useState(Date.now);
  const [selection, setSelection] =
    useState<DashboardConversationSelection | null>(null);
  const [conversationMounted, setConversationMounted] = useState(false);
  useEffect(() => {
    const timer = setInterval(() => {
      if (document.visibilityState !== 'hidden') setNow(Date.now());
    }, 10_000);
    return () => clearInterval(timer);
  }, []);
  const onOpen = useCallback(
    (next: DashboardConversationSelection) => {
      if (isMobile) {
        const href = dashboardTargetHref(scope.hostId, next.target, next.owner);
        if (href) scope.navigateToRoute(href);
      } else {
        setConversationMounted(true);
        setSelection(next);
      }
    },
    [isMobile, scope]
  );
  const onNavigate = useCallback(
    (target: DashboardOpenTarget, owner: DashboardWorkRow) => {
      const href = dashboardTargetHref(scope.hostId, target, owner);
      if (href) scope.navigateToRoute(href);
    },
    [scope]
  );
  const closeConversation = useCallback(() => {
    const trigger = selection?.trigger;
    setSelection(null);
    requestAnimationFrame(() => {
      if (trigger?.isConnected) trigger.focus({ preventScroll: true });
    });
  }, [selection]);
  const data = dashboard.data;
  const expandedKeys = useMemo(
    () =>
      new Set(
        dashboard.view.expanded.map((page) => dashboardWorkKey(page.work))
      ),
    [dashboard.view.expanded]
  );
  return (
    <div
      className="session-dashboard"
      role="region"
      aria-labelledby={titleId}
      data-testid="dashboard-page"
    >
      <header className="session-dashboard__header">
        <h1 id={titleId}>{t('sessionDashboard.title')}</h1>
        <div className="session-dashboard__header-actions">
          {dashboard.lastSuccess ? (
            <span className="session-dashboard__muted">
              {t('sessionDashboard.updated')}{' '}
              <DashboardTime at={dashboard.lastSuccess} now={now} />
            </span>
          ) : null}
          <button
            type="button"
            className="session-dashboard__text-button"
            disabled={dashboard.isRefreshing || !hostAvailable}
            onClick={dashboard.refresh}
          >
            <RefreshCw size={14} aria-hidden="true" />
            {t('sessionDashboard.refresh')}
          </button>
        </div>
      </header>
      <DashboardToolbar
        filters={dashboard.view.filters}
        projects={data?.projects ?? []}
        providers={data?.providers ?? []}
        onChange={dashboard.setFilters}
      />
      {data ? (
        <div className="session-dashboard__summary">
          <span>
            {t('sessionDashboard.workCount', { count: data.counts.work_count })}
            <span aria-hidden="true"> · </span>
            {t('sessionDashboard.activeSessionCount', {
              count: data.counts.active_session_count,
            })}
          </span>
          {data.counts.active_run_count > data.counts.active_session_count ? (
            <span className="session-dashboard__issue">
              {t('sessionDashboard.multipleRuns', {
                count: data.counts.active_run_count,
              })}
            </span>
          ) : null}
          {data.has_updates ||
          data.expanded.some((page) => page.has_updates || page.invalidation) ||
          data.snapshot_status !== 'ready' ? (
            <button
              type="button"
              className="session-dashboard__update-button"
              onClick={dashboard.refresh}
            >
              <RefreshCw size={12} aria-hidden="true" />
              {t(
                data.snapshot_status === 'expired'
                  ? 'sessionDashboard.snapshotExpired'
                  : data.snapshot_status === 'capacity'
                    ? 'sessionDashboard.snapshotCapacity'
                    : 'sessionDashboard.hasUpdates'
              )}
            </button>
          ) : null}
        </div>
      ) : null}
      {dashboard.error && data ? (
        <div className="session-dashboard__notice" role="status">
          <AlertTriangle size={14} aria-hidden="true" />
          <span>
            {t('sessionDashboard.refreshFailed')}
            {dashboard.lastSuccess ? (
              <>
                {' '}
                · {t('sessionDashboard.lastSuccess')}{' '}
                <DashboardTime at={dashboard.lastSuccess} now={now} />
              </>
            ) : null}
          </span>
          <button
            type="button"
            className="session-dashboard__text-button"
            onClick={dashboard.refresh}
          >
            {t('buttons.retry')}
          </button>
        </div>
      ) : null}
      {data?.issues.length ? (
        <details className="session-dashboard__partial">
          <summary>
            {t('sessionDashboard.partial', { count: data.issues.length })}
          </summary>
          {data.issues.map((issue, index) => (
            <p key={`${issue.code}:${index}`}>{issue.message}</p>
          ))}
        </details>
      ) : null}
      {!hostAvailable ? (
        <ErrorState
          className="min-h-0 flex-1"
          title={t('sessionDashboard.noHostTitle')}
          description={t('sessionDashboard.noHostDescription')}
        />
      ) : dashboard.isLoading ? (
        <LoadingState
          className="min-h-0 flex-1"
          title={t('sessionDashboard.loading')}
        />
      ) : dashboard.error && !data ? (
        <ErrorState
          className="min-h-0 flex-1"
          title={t('sessionDashboard.loadFailed')}
          description={dashboard.error.message}
          action={
            <Button onClick={dashboard.refresh}>{t('buttons.retry')}</Button>
          }
        />
      ) : data ? (
        <>
          <DashboardTable
            key={`${scope.scopeKey}:${data.page}`}
            data={data}
            groupByProject={dashboard.view.filters.group_by_project}
            expandedKeys={expandedKeys}
            scopeKey={scope.scopeKey}
            hostId={scope.hostId}
            now={now}
            isRefreshing={dashboard.isRefreshing}
            scrollTop={dashboard.view.scrollTop}
            selectedSessionId={selection?.target.session_id}
            onScroll={dashboard.saveScroll}
            onToggle={dashboard.toggleExpanded}
            onChildPage={dashboard.setChildPage}
            onOpen={onOpen}
            onNavigate={onNavigate}
          />
          <Pagination
            page={data.page}
            pageSize={data.page_size}
            total={data.total}
            disabled={dashboard.isRefreshing}
            onPage={dashboard.setPage}
          />
        </>
      ) : null}
      {conversationMounted ? (
        <Suspense
          fallback={
            <span className="session-dashboard__panel-loading" role="status">
              {t('sessionDashboard.loading')}
            </span>
          }
        >
          <DashboardConversationPanel
            selection={selection}
            scopeKey={scope.scopeKey}
            hostId={scope.hostId}
            onClose={closeConversation}
            onNavigate={scope.navigateToRoute}
          />
        </Suspense>
      ) : null}
    </div>
  );
}
