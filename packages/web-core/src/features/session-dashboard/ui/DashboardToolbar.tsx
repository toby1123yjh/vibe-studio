import { useEffect, useState } from 'react';
import { Search, SlidersHorizontal, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@vibe/ui/components/Popover';
import type {
  DashboardFilters,
  DashboardNamedRef,
  DashboardWorkKind,
} from 'shared/types';
import {
  agentExecutorFromSearch,
  AGENT_PROVIDER_BY_EXECUTOR,
} from '@/shared/lib/agentProviders';
import {
  dashboardProjectTitle,
  DEFAULT_DASHBOARD_FILTERS,
} from '../model/dashboard';

export function DashboardToolbar({
  filters,
  projects,
  providers,
  onChange,
}: {
  filters: DashboardFilters;
  projects: DashboardNamedRef[];
  providers: string[];
  onChange: (filters: DashboardFilters) => void;
}) {
  const { t } = useTranslation('common');
  const [search, setSearch] = useState(filters.search);
  useEffect(() => setSearch(filters.search), [filters.search]);
  useEffect(() => {
    if (search.trim() === filters.search) return;
    const timer = setTimeout(
      () => onChange({ ...filters, search: search.trim() }),
      250
    );
    return () => clearTimeout(timer);
  }, [filters, onChange, search]);
  const kinds: DashboardWorkKind[] = [
    'task',
    'workflow',
    'session',
    'arena',
    'unresolved',
  ];
  const activeFilters = [
    filters.kinds.length > 0,
    filters.project_id !== null,
    filters.provider_id !== null,
    filters.directory_kind !== null,
    filters.group_by_project !== DEFAULT_DASHBOARD_FILTERS.group_by_project,
  ].filter(Boolean).length;
  return (
    <div className="session-dashboard__toolbar">
      <div className="session-dashboard__toolbar-actions">
        <label className="session-dashboard__search">
          <Search size={14} aria-hidden="true" />
          <input
            aria-label={t('sessionDashboard.filters.search')}
            placeholder={t('sessionDashboard.filters.searchPlaceholder')}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
          {search ? (
            <button
              type="button"
              aria-label={t('sessionDashboard.filters.clearSearch')}
              onClick={() => setSearch('')}
            >
              <X size={13} aria-hidden="true" />
            </button>
          ) : null}
        </label>
        <Popover>
          <PopoverTrigger asChild>
            <button
              type="button"
              className="session-dashboard__filter-trigger"
              aria-label={t('sessionDashboard.filters.open')}
              data-active={activeFilters > 0}
            >
              <SlidersHorizontal size={14} aria-hidden="true" />
              {t('sessionDashboard.filters.open')}
              {activeFilters > 0 ? (
                <span
                  className="session-dashboard__filter-count"
                  aria-hidden="true"
                >
                  {activeFilters}
                </span>
              ) : null}
            </button>
          </PopoverTrigger>
          <PopoverContent
            align="end"
            sideOffset={8}
            className="session-dashboard__filter-panel"
            role="dialog"
            aria-label={t('sessionDashboard.filters.open')}
          >
            <div className="session-dashboard__filter-heading">
              <span>{t('sessionDashboard.filters.open')}</span>
              <button
                type="button"
                disabled={activeFilters === 0}
                onClick={() =>
                  onChange({
                    ...DEFAULT_DASHBOARD_FILTERS,
                    search: filters.search,
                  })
                }
              >
                {t('sessionDashboard.filters.reset')}
              </button>
            </div>
            <div className="session-dashboard__filter-fields">
              <label>
                <span>{t('sessionDashboard.filters.kind')}</span>
                <select
                  aria-label={t('sessionDashboard.filters.kind')}
                  value={filters.kinds[0] ?? ''}
                  onChange={(event) =>
                    onChange({
                      ...filters,
                      kinds: event.target.value
                        ? [event.target.value as DashboardWorkKind]
                        : [],
                    })
                  }
                >
                  <option value="">
                    {t('sessionDashboard.filters.allKinds')}
                  </option>
                  {kinds.map((kind) => (
                    <option key={kind} value={kind}>
                      {t(`sessionDashboard.kinds.${kind}`)}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>{t('sessionDashboard.filters.project')}</span>
                <select
                  aria-label={t('sessionDashboard.filters.project')}
                  value={filters.project_id ?? ''}
                  onChange={(event) =>
                    onChange({
                      ...filters,
                      project_id: event.target.value || null,
                    })
                  }
                >
                  <option value="">
                    {t('sessionDashboard.filters.allProjects')}
                  </option>
                  {projects.map((project) => (
                    <option key={project.id} value={project.id}>
                      {dashboardProjectTitle(project, t)}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>{t('sessionDashboard.filters.agent')}</span>
                <select
                  aria-label={t('sessionDashboard.filters.agent')}
                  value={filters.provider_id ?? ''}
                  onChange={(event) =>
                    onChange({
                      ...filters,
                      provider_id: event.target.value || null,
                    })
                  }
                >
                  <option value="">
                    {t('sessionDashboard.filters.allAgents')}
                  </option>
                  {providers.map((provider) => {
                    const executor = agentExecutorFromSearch(provider);
                    return (
                      <option key={provider} value={provider}>
                        {executor
                          ? AGENT_PROVIDER_BY_EXECUTOR[executor].label
                          : provider}
                      </option>
                    );
                  })}
                </select>
              </label>
              <label>
                <span>{t('sessionDashboard.filters.directory')}</span>
                <select
                  aria-label={t('sessionDashboard.filters.directory')}
                  value={filters.directory_kind ?? ''}
                  onChange={(event) =>
                    onChange({
                      ...filters,
                      directory_kind: event.target.value || null,
                    })
                  }
                >
                  <option value="">
                    {t('sessionDashboard.filters.allDirectories')}
                  </option>
                  <option value="direct_folder">
                    {t('sessionDashboard.directory.direct_folder')}
                  </option>
                  <option value="worktree">
                    {t('sessionDashboard.directory.worktree')}
                  </option>
                  <option value="unknown">
                    {t('sessionDashboard.directory.unknown')}
                  </option>
                </select>
              </label>
            </div>
            <label className="session-dashboard__group-toggle">
              <input
                type="checkbox"
                checked={filters.group_by_project}
                onChange={(event) =>
                  onChange({
                    ...filters,
                    group_by_project: event.target.checked,
                  })
                }
              />
              {t('sessionDashboard.filters.groupProject')}
            </label>
          </PopoverContent>
        </Popover>
      </div>
    </div>
  );
}
