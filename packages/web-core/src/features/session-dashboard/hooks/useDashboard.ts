import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { replaceEqualDeep, useQueryClient } from '@tanstack/react-query';
import type {
  DashboardExpandedPage,
  DashboardFilters,
  DashboardPageResponse,
  DashboardWorkRow,
} from 'shared/types';
import { createDashboardApi } from '@/shared/lib/dashboardApi';
import {
  DEFAULT_DASHBOARD_FILTERS,
  dashboardQueryKey,
  dashboardWorkKey,
  reconcileDashboardExpanded,
} from '../model/dashboard';

interface DashboardView {
  filters: DashboardFilters;
  page: number;
  expanded: DashboardExpandedPage[];
  snapshotId: string | null;
  scrollTop: number;
}

// Only presentation state lives here. Canonical rows remain in the query cache.
const savedViews = new Map<string, DashboardView>();
const emptyView = (): DashboardView => ({
  filters: { ...DEFAULT_DASHBOARD_FILTERS },
  page: 0,
  expanded: [],
  snapshotId: null,
  scrollTop: 0,
});

export function useDashboard(
  scopeKey: string,
  hostId: string | null,
  enabled = true
) {
  const queryClient = useQueryClient();
  const api = useMemo(() => createDashboardApi(hostId), [hostId]);
  const [view, setView] = useState<DashboardView>(
    () => savedViews.get(scopeKey) ?? emptyView()
  );
  const viewRef = useRef(view);
  const [refreshEpoch, setRefreshEpoch] = useState(0);
  const [data, setData] = useState<DashboardPageResponse>();
  const dataRef = useRef<DashboardPageResponse>();
  const [error, setError] = useState<Error | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [lastSuccess, setLastSuccess] = useState<string | null>(null);
  const forceRefresh = useRef(false);
  const requestGeneration = useRef(0);
  const saveView = useCallback(
    (next: DashboardView) => {
      viewRef.current = next;
      savedViews.delete(scopeKey);
      savedViews.set(scopeKey, next);
      if (savedViews.size > 8) {
        const oldest = savedViews.keys().next().value;
        if (oldest) savedViews.delete(oldest);
      }
      setView(next);
    },
    [scopeKey]
  );
  const filtersIdentity = JSON.stringify(view.filters);
  const expandedIdentity = JSON.stringify(
    view.expanded.map((entry) => [dashboardWorkKey(entry.work), entry.page])
  );

  useEffect(() => {
    if (!enabled) return;
    const generation = ++requestGeneration.current;
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let running = false;
    let failures = 0;
    let currentKey: ReturnType<typeof dashboardQueryKey> | null = null;
    const current = () =>
      !abort.signal.aborted && requestGeneration.current === generation;

    async function poll() {
      if (!current() || running || document.visibilityState === 'hidden')
        return;
      running = true;
      setIsRefreshing(true);
      const capturedView = viewRef.current;
      const refresh = forceRefresh.current;
      const snapshotId = refresh ? null : capturedView.snapshotId;
      currentKey = dashboardQueryKey(
        scopeKey,
        capturedView.filters,
        snapshotId,
        capturedView.page,
        capturedView.expanded
      );
      try {
        const response = await queryClient.fetchQuery({
          queryKey: currentKey,
          staleTime: 0,
          gcTime: 5 * 60_000,
          retry: false,
          queryFn: () =>
            api.query(
              {
                filters: capturedView.filters,
                snapshot_id: snapshotId,
                page: capturedView.page,
                page_size: 20,
                expanded: capturedView.expanded,
                visible: dataRef.current?.rows.map((row) => row.work) ?? [],
                refresh,
              },
              abort.signal
            ),
        });
        if (!current()) return;
        const lastPage = Math.max(
          0,
          Math.ceil(response.total / response.page_size) - 1
        );
        if (capturedView.page > lastPage) {
          saveView({
            ...viewRef.current,
            page: lastPage,
            expanded: [],
            scrollTop: 0,
          });
          return;
        }
        forceRefresh.current = false;
        failures = 0;
        const sharedResponse = replaceEqualDeep(dataRef.current, response);
        dataRef.current = sharedResponse;
        setData(sharedResponse);
        setLastSuccess(response.generated_at);
        setError(null);
        const expanded = reconcileDashboardExpanded(
          viewRef.current.expanded,
          response.rows
        ).map((entry) => {
          const childPage = response.expanded.find(
            (page) =>
              dashboardWorkKey(page.work) === dashboardWorkKey(entry.work)
          );
          if (
            !childPage ||
            childPage.workflow_run_id !== entry.workflow_run_id
          ) {
            return entry;
          }
          return {
            ...entry,
            snapshot_id: childPage.invalidation ? null : childPage.snapshot_id,
            page: childPage.invalidation ? 0 : childPage.page,
          };
        });
        saveView({
          ...viewRef.current,
          snapshotId: response.snapshot_id ?? viewRef.current.snapshotId,
          expanded,
        });
      } catch (cause) {
        if (!current()) return;
        failures += 1;
        setError(cause instanceof Error ? cause : new Error(String(cause)));
      } finally {
        running = false;
        if (current()) {
          setIsRefreshing(false);
          timer = setTimeout(
            () => void poll(),
            Math.min(15_000, 2000 * 2 ** failures)
          );
        }
      }
    }

    const onVisibility = () => {
      if (timer) clearTimeout(timer);
      if (document.visibilityState !== 'hidden') void poll();
    };
    document.addEventListener('visibilitychange', onVisibility);
    void poll();
    return () => {
      abort.abort();
      if (timer) clearTimeout(timer);
      if (currentKey)
        void queryClient.cancelQueries({ queryKey: currentKey, exact: true });
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [
    api,
    enabled,
    expandedIdentity,
    filtersIdentity,
    queryClient,
    refreshEpoch,
    saveView,
    scopeKey,
    view.page,
  ]);

  const refresh = useCallback(() => {
    forceRefresh.current = true;
    saveView({
      ...viewRef.current,
      page: 0,
      snapshotId: null,
      expanded: viewRef.current.expanded.map((entry) => ({
        ...entry,
        snapshot_id: null,
        page: 0,
      })),
    });
    setRefreshEpoch((value) => value + 1);
  }, [saveView]);
  const setFilters = useCallback(
    (filters: DashboardFilters) => {
      dataRef.current = undefined;
      setData(undefined);
      setError(null);
      saveView({
        filters,
        page: 0,
        snapshotId: null,
        expanded: [],
        scrollTop: 0,
      });
    },
    [saveView]
  );
  const setPage = useCallback(
    (page: number) => {
      saveView({ ...viewRef.current, page, expanded: [], scrollTop: 0 });
    },
    [saveView]
  );
  const toggleExpanded = useCallback(
    (row: DashboardWorkRow) => {
      const entries = viewRef.current.expanded;
      const key = dashboardWorkKey(row.work);
      const existing = entries.some(
        (entry) => dashboardWorkKey(entry.work) === key
      );
      saveView({
        ...viewRef.current,
        expanded: existing
          ? entries.filter((entry) => dashboardWorkKey(entry.work) !== key)
          : [
              ...entries,
              {
                work: row.work,
                workflow_run_id: row.workflow_run_id,
                snapshot_id: null,
                page: 0,
              },
            ],
      });
    },
    [saveView]
  );
  const setChildPage = useCallback(
    (workKey: string, page: number) => {
      saveView({
        ...viewRef.current,
        expanded: viewRef.current.expanded.map((entry) =>
          dashboardWorkKey(entry.work) === workKey ? { ...entry, page } : entry
        ),
      });
    },
    [saveView]
  );
  const saveScroll = useCallback(
    (scrollTop: number) => {
      const next = { ...viewRef.current, scrollTop };
      viewRef.current = next;
      savedViews.set(scopeKey, next);
    },
    [scopeKey]
  );

  return {
    data,
    error,
    isRefreshing,
    isLoading: enabled && !data && !error,
    lastSuccess,
    view,
    refresh,
    setFilters,
    setPage,
    toggleExpanded,
    setChildPage,
    saveScroll,
  };
}
