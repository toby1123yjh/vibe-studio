import {
  Fragment,
  memo,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import {
  Activity,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  GitBranch,
  Info,
  MessageSquare,
  RefreshCw,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type {
  DashboardAgent,
  DashboardChildRow,
  DashboardChildrenPage,
  DashboardOpenTarget,
  DashboardPageResponse,
  DashboardReplyPreview,
  DashboardWorkRow,
} from 'shared/types';
import { AgentIcon } from '@/shared/components/AgentIcon';
import {
  agentExecutorFromSearch,
  AGENT_PROVIDER_BY_EXECUTOR,
  getAgentBrandAttributes,
} from '@/shared/lib/agentProviders';
import {
  dashboardActivityKey,
  dashboardAge,
  dashboardProjectTitle,
  dashboardRowKey,
  dashboardStatusTone,
  dashboardWorkKey,
} from '../model/dashboard';
import { DashboardContext } from './DashboardContext';

export interface DashboardConversationSelection {
  target: Extract<DashboardOpenTarget, { kind: 'session' }>;
  owner: DashboardWorkRow;
  title: string;
  status: string;
  role: string;
  readOnly: boolean;
  trigger: HTMLElement | null;
}

export function DashboardTime({
  at,
  now,
}: {
  at: string | null | undefined;
  now: number;
}) {
  const { t, i18n } = useTranslation('common');
  const age = dashboardAge(at, now);
  return (
    <time
      dateTime={at ?? undefined}
      title={at && age ? new Date(at).toLocaleString(i18n.language) : undefined}
    >
      {age ? t(`sessionDashboard.time.${age.unit}`, { count: age.value }) : '—'}
    </time>
  );
}

function AgentIdentity({ agent }: { agent: DashboardAgent }) {
  const { t } = useTranslation('common');
  const executor = agentExecutorFromSearch(agent.provider_id);
  const details = [agent.runtime_profile_id, agent.model_id, agent.reasoning_id]
    .filter(Boolean)
    .join(' · ');
  return (
    <span
      className="session-dashboard__agent"
      {...getAgentBrandAttributes(executor)}
      title={`${details || t('sessionDashboard.unavailable')} (${t(`sessionDashboard.configuration.${agent.configuration_source}`, { defaultValue: agent.configuration_source })})`}
    >
      {executor ? <AgentIcon agent={executor} className="h-3.5 w-3.5" /> : null}
      <span className="vk-agent-brand-label">
        {executor
          ? AGENT_PROVIDER_BY_EXECUTOR[executor].label
          : agent.provider_id}
      </span>
    </span>
  );
}

const statusKeys: Record<string, string> = {
  pending: 'pending',
  starting: 'starting',
  running: 'running',
  awaiting_input: 'awaiting_input',
  awaiting_approval: 'awaiting_approval',
  awaiting_human: 'awaiting_human',
  awaiting_arena: 'awaiting_arena',
  cancelling: 'cancelling',
  succeeded: 'succeeded',
  failed: 'failed',
  cancelled: 'cancelled',
  canceled: 'cancelled',
  crashed: 'crashed',
  audit_failed: 'audit_failed',
  not_started: 'idle',
  draft: 'draft',
  ready: 'ready',
  open: 'open',
  closed: 'closed',
  adopted: 'adopted',
  implementation_started: 'implementation_started',
  reused: 'reused',
  skipped: 'skipped',
  unavailable: 'unavailable',
};

function DashboardStatus({
  rawStatus,
  group,
  now,
  updatedAt,
  waitingCount,
}: {
  rawStatus: string;
  group: string;
  now: number;
  updatedAt: string;
  waitingCount?: number;
}) {
  const { t } = useTranslation('common');
  return (
    <div className="session-dashboard__status-cell">
      <span
        className="session-dashboard__status"
        data-tone={dashboardStatusTone(group)}
        title={rawStatus}
      >
        <span className="session-dashboard__status-dot" aria-hidden="true" />
        {t(`sessionDashboard.statuses.${statusKeys[rawStatus] ?? 'unknown'}`)}
      </span>
      {waitingCount ? (
        <span className="session-dashboard__muted">
          {t('sessionDashboard.waitingCount', { count: waitingCount })}
        </span>
      ) : null}
      <span className="session-dashboard__muted">
        <DashboardTime at={updatedAt} now={now} />
      </span>
    </div>
  );
}

function Reply({
  preview,
  previewStatus,
  activity,
  now,
  onReply,
}: {
  preview: DashboardReplyPreview | null;
  previewStatus: string;
  activity: DashboardWorkRow['activity'];
  now: number;
  onReply: (preview: DashboardReplyPreview, trigger: HTMLElement) => void;
}) {
  const { t } = useTranslation('common');
  return (
    <div className="session-dashboard__reply">
      {preview ? (
        <button
          type="button"
          className="session-dashboard__reply-button"
          title={preview.text}
          onClick={(event) => onReply(preview, event.currentTarget)}
        >
          <span className="session-dashboard__reply-text">
            {preview.text}
            {preview.truncated ? '…' : ''}
          </span>
          <span className="session-dashboard__muted session-dashboard__source">
            {preview.source_title ||
              t(`sessionDashboard.roles.${preview.source_role}`, {
                defaultValue: preview.source_role,
              })}
            <span aria-hidden="true"> · </span>
            <DashboardTime at={preview.occurred_at} now={now} />
            {!preview.caught_up ? (
              <span> · {t('sessionDashboard.preview.catching_up')}</span>
            ) : null}
          </span>
        </button>
      ) : (
        <span className="session-dashboard__muted">
          {t(
            `sessionDashboard.preview.${['catching_up', 'rebuilding', 'unavailable', 'missing'].includes(previewStatus) ? previewStatus : 'missing'}`
          )}
        </span>
      )}
      {activity ? (
        <span
          className="session-dashboard__activity"
          title={activity.description ?? activity.kind}
        >
          <Activity size={11} aria-hidden="true" />
          <span>
            {t(dashboardActivityKey(activity.kind, activity.status), {
              defaultValue: t('sessionDashboard.activities.unknown'),
            })}
            {activity.description ? ` · ${activity.description}` : ''}
          </span>
        </span>
      ) : null}
    </div>
  );
}

interface WorkRowProps {
  row: DashboardWorkRow;
  child?: DashboardChildRow;
  scopeKey: string;
  hostId: string | null;
  expanded?: boolean;
  now: number;
  selectedSessionId?: string;
  onToggle?: (row: DashboardWorkRow) => void;
  onOpen: (selection: DashboardConversationSelection) => void;
  onNavigate: (target: DashboardOpenTarget, owner: DashboardWorkRow) => void;
}

const WorkRow = memo(function WorkRow({
  row,
  child,
  scopeKey,
  hostId,
  expanded,
  now,
  selectedSessionId,
  onToggle,
  onOpen,
  onNavigate,
}: WorkRowProps) {
  const { t } = useTranslation('common');
  const [contextOpen, setContextOpen] = useState(false);
  const source = child ?? row;
  const target = source.open_target;
  const mode = child?.role === 'arena_candidate' ? 'arena' : row.work.kind;
  const open = (
    targetToOpen: DashboardOpenTarget,
    trigger: HTMLElement,
    preview?: DashboardReplyPreview
  ) => {
    if (targetToOpen.kind !== 'session') {
      onNavigate(targetToOpen, row);
      return;
    }
    onOpen({
      target: targetToOpen,
      owner: row,
      title: preview?.source_title || source.title,
      status: source.raw_status,
      role: preview?.source_role || child?.role || 'session',
      readOnly: child?.reused ?? false,
      trigger,
    });
  };
  const agents = child ? (child.agent ? [child.agent] : []) : row.agents;
  const actualSession =
    child?.session_id ??
    (target?.kind === 'session' ? target.session_id : undefined);
  return (
    <>
      <tr
        className="session-dashboard__work-row"
        data-mode={mode}
        data-child={Boolean(child)}
        data-selected={Boolean(
          actualSession && selectedSessionId === actualSession
        )}
        data-invalidated={!child && row.invalidated}
        data-testid="dashboard-work-row"
        data-work-key={child?.id ?? dashboardWorkKey(row.work)}
      >
        <td className="session-dashboard__status-column">
          <DashboardStatus
            rawStatus={source.raw_status}
            group={source.status_group}
            now={now}
            updatedAt={source.activity?.occurred_at ?? source.updated_at}
            waitingCount={!child ? row.waiting_count : undefined}
          />
          {child?.agent_run_status &&
          child.agent_run_status !== child.raw_status ? (
            <span
              className="session-dashboard__muted"
              title={child.agent_run_status}
            >
              {t('sessionDashboard.agentRunStatus', {
                status: t(
                  `sessionDashboard.statuses.${statusKeys[child.agent_run_status] ?? 'unknown'}`
                ),
              })}
            </span>
          ) : null}
        </td>
        <td className="session-dashboard__kind-column">
          <span className="session-dashboard__kind">
            {t(`sessionDashboard.kinds.${mode}`)}
          </span>
          {child ? (
            <span className="session-dashboard__role">
              {t(`sessionDashboard.roles.${child.role}`, {
                defaultValue: child.role,
              })}
            </span>
          ) : null}
        </td>
        <td className="session-dashboard__work-column">
          <div className="session-dashboard__work-title">
            {!child && row.child_count > 0 ? (
              <button
                type="button"
                className="session-dashboard__icon-button"
                aria-expanded={expanded}
                aria-label={t(
                  expanded
                    ? 'sessionDashboard.collapse'
                    : 'sessionDashboard.expand',
                  { title: row.title }
                )}
                onClick={() => onToggle?.(row)}
              >
                {expanded ? (
                  <ChevronDown size={15} aria-hidden="true" />
                ) : (
                  <ChevronRight size={15} aria-hidden="true" />
                )}
              </button>
            ) : child ? (
              <span
                className="session-dashboard__tree-branch"
                aria-hidden="true"
              />
            ) : null}
            {target && !row.invalidated ? (
              <button
                type="button"
                className="session-dashboard__title-button"
                title={source.title}
                onClick={(event) => open(target, event.currentTarget)}
              >
                {source.title}
              </button>
            ) : (
              <span className="session-dashboard__title" title={source.title}>
                {source.title}
              </span>
            )}
          </div>
          {!child && row.task && row.task.title !== row.title ? (
            <p
              className="session-dashboard__muted session-dashboard__task-title"
              title={row.task.title ?? row.task.id}
            >
              {row.task.title ?? row.task.id.slice(0, 8)}
            </p>
          ) : null}
          {!child && row.active_session_count > 0 ? (
            <p className="session-dashboard__muted">
              {t('sessionDashboard.sessionCount', {
                count: row.active_session_count,
              })}
              {row.matching_session_count !== row.active_session_count
                ? ` · ${t('sessionDashboard.matchingCount', { count: row.matching_session_count })}`
                : ''}
            </p>
          ) : null}
          {!child && row.stale_membership ? (
            <p className="session-dashboard__issue">
              {t('sessionDashboard.pendingRefresh')}
            </p>
          ) : null}
          {child?.reused ? (
            <p className="session-dashboard__muted">
              {t('sessionDashboard.reused')}
            </p>
          ) : null}
          {child && !child.matches_filter ? (
            <p className="session-dashboard__muted">
              {t('sessionDashboard.otherWork')}
            </p>
          ) : null}
          <div className="session-dashboard__mobile-meta">
            {row.project ? dashboardProjectTitle(row.project, t) : null}
            {source.directory.path ? (
              <span title={source.directory.path}>{source.directory.path}</span>
            ) : null}
          </div>
        </td>
        <td className="session-dashboard__directory-column">
          <span className="session-dashboard__project-name">
            {dashboardProjectTitle(row.project, t)}
          </span>
          <span
            className="session-dashboard__path"
            title={source.directory.path ?? undefined}
          >
            {source.directory.path ?? t('sessionDashboard.directory.unknown')}
          </span>
          <span className="session-dashboard__muted session-dashboard__branch">
            {source.directory.git_branch ? (
              <>
                <GitBranch size={11} aria-hidden="true" />
                {source.directory.git_branch}
              </>
            ) : (
              t(`sessionDashboard.directory.${source.directory.kind}`, {
                defaultValue: source.directory.kind,
              })
            )}
          </span>
        </td>
        <td className="session-dashboard__agent-column">
          <div className="session-dashboard__agents">
            {agents.length ? (
              agents.map((agent) => (
                <AgentIdentity
                  key={JSON.stringify([
                    agent.provider_id,
                    agent.runtime_profile_id,
                    agent.model_id,
                    agent.reasoning_id,
                    agent.configuration_source,
                  ])}
                  agent={agent}
                />
              ))
            ) : (
              <span className="session-dashboard__muted">
                {t('sessionDashboard.noAgent')}
              </span>
            )}
          </div>
        </td>
        <td className="session-dashboard__reply-column">
          <Reply
            preview={source.preview}
            previewStatus={source.preview_status}
            activity={source.activity}
            now={now}
            onReply={(preview, trigger) =>
              open(preview.open_target, trigger, preview)
            }
          />
        </td>
        <td className="session-dashboard__actions-column">
          <div className="session-dashboard__actions">
            <button
              type="button"
              className="session-dashboard__icon-button"
              aria-expanded={contextOpen}
              aria-label={t('sessionDashboard.context.show', {
                title: source.title,
              })}
              title={t('sessionDashboard.context.title')}
              onClick={() => setContextOpen((value) => !value)}
            >
              <Info size={15} aria-hidden="true" />
            </button>
            {target && !row.invalidated ? (
              <button
                type="button"
                className="session-dashboard__icon-button"
                aria-label={t(
                  target.kind === 'session'
                    ? 'sessionDashboard.openConversation'
                    : 'sessionDashboard.openWork',
                  { title: source.title }
                )}
                title={t(
                  target.kind === 'session'
                    ? 'sessionDashboard.openConversation'
                    : 'sessionDashboard.openWork',
                  { title: source.title }
                )}
                onClick={(event) => open(target, event.currentTarget)}
              >
                {target.kind === 'session' ? (
                  <MessageSquare size={15} aria-hidden="true" />
                ) : (
                  <ExternalLink size={15} aria-hidden="true" />
                )}
              </button>
            ) : null}
          </div>
        </td>
      </tr>
      {source.issues.length ? (
        <tr className="session-dashboard__issues-row">
          <td colSpan={7}>
            {source.issues.map((issue, index) => (
              <span key={`${issue.code}:${index}`}>{issue.message}</span>
            ))}
          </td>
        </tr>
      ) : null}
      {contextOpen ? (
        <tr className="session-dashboard__context-row">
          <td colSpan={7}>
            <DashboardContext
              work={row.work}
              hostId={hostId}
              scopeKey={scopeKey}
              sessionId={
                child?.session_id ??
                (target?.kind === 'session' ? target.session_id : undefined)
              }
              agentRunId={
                child?.agent_run_id ??
                (target?.kind === 'session'
                  ? (target.agent_run_id ?? undefined)
                  : undefined)
              }
            />
          </td>
        </tr>
      ) : null}
    </>
  );
});

function ChildRows({
  row,
  page,
  loading,
  onPage,
  ...props
}: Omit<WorkRowProps, 'child'> & {
  page?: DashboardChildrenPage;
  loading: boolean;
  onPage: (page: number) => void;
}) {
  const { t } = useTranslation('common');
  if (!page || page.workflow_run_id !== row.workflow_run_id)
    return (
      <tr className="session-dashboard__child-state">
        <td colSpan={7} role="status">
          {t(
            loading
              ? 'sessionDashboard.loadingChildren'
              : 'sessionDashboard.childrenPending'
          )}
        </td>
      </tr>
    );
  return (
    <>
      {page.invalidation ? (
        <tr className="session-dashboard__child-state">
          <td colSpan={7} role="status">
            {t('sessionDashboard.childrenChanged')}
          </td>
        </tr>
      ) : null}
      {page.rows.map((child) => (
        <WorkRow
          key={dashboardRowKey(props.hostId, row.work, child)}
          {...props}
          row={row}
          child={child}
        />
      ))}
      {page.total > page.page_size ? (
        <tr className="session-dashboard__child-state">
          <td colSpan={7}>
            <Pagination
              page={page.page}
              pageSize={page.page_size}
              total={page.total}
              disabled={loading}
              onPage={onPage}
              child
            />
          </td>
        </tr>
      ) : null}
    </>
  );
}

export function Pagination({
  page,
  pageSize,
  total,
  disabled,
  onPage,
  child = false,
}: {
  page: number;
  pageSize: number;
  total: number;
  disabled?: boolean;
  onPage: (page: number) => void;
  child?: boolean;
}) {
  const { t } = useTranslation('common');
  const pages = Math.max(1, Math.ceil(total / Math.max(1, pageSize)));
  return (
    <nav
      className="session-dashboard__pagination"
      aria-label={t(
        child
          ? 'sessionDashboard.childPagination'
          : 'sessionDashboard.pagination'
      )}
    >
      <span>
        {t('sessionDashboard.page', { page: page + 1, pages, total })}
      </span>
      <button
        type="button"
        className="session-dashboard__icon-button"
        disabled={page === 0 || disabled}
        aria-label={t('sessionDashboard.previousPage')}
        onClick={() => onPage(page - 1)}
      >
        <ChevronLeft size={16} aria-hidden="true" />
      </button>
      <button
        type="button"
        className="session-dashboard__icon-button"
        disabled={page >= pages - 1 || disabled}
        aria-label={t('sessionDashboard.nextPage')}
        onClick={() => onPage(page + 1)}
      >
        <ChevronRight size={16} aria-hidden="true" />
      </button>
    </nav>
  );
}

export function DashboardTable({
  data,
  groupByProject,
  expandedKeys,
  scopeKey,
  hostId,
  now,
  isRefreshing,
  scrollTop,
  selectedSessionId,
  onScroll,
  onToggle,
  onChildPage,
  onOpen,
  onNavigate,
}: {
  data: DashboardPageResponse;
  groupByProject: boolean;
  expandedKeys: ReadonlySet<string>;
  scopeKey: string;
  hostId: string | null;
  now: number;
  isRefreshing: boolean;
  scrollTop: number;
  selectedSessionId?: string;
  onScroll: (scrollTop: number) => void;
  onToggle: (row: DashboardWorkRow) => void;
  onChildPage: (key: string, page: number) => void;
  onOpen: WorkRowProps['onOpen'];
  onNavigate: WorkRowProps['onNavigate'];
}) {
  const { t } = useTranslation('common');
  const scroller = useRef<HTMLDivElement>(null);
  const initialScrollTop = useRef(scrollTop);
  useLayoutEffect(() => {
    if (scroller.current) scroller.current.scrollTop = initialScrollTop.current;
    // Restore only on mount; streaming updates must never move the viewport.
  }, []);
  let previousProject: string | null | undefined;
  const rows: ReactNode[] = [];
  for (const row of data.rows) {
    const key = dashboardWorkKey(row.work);
    const project = row.project?.id ?? null;
    if (groupByProject && previousProject !== project) {
      rows.push(
        <tr
          key={`project:${project}:${key}`}
          className="session-dashboard__project-group"
        >
          <th scope="rowgroup" colSpan={7}>
            {dashboardProjectTitle(row.project, t)}
          </th>
        </tr>
      );
    }
    previousProject = project;
    rows.push(
      <Fragment key={dashboardRowKey(hostId, row.work)}>
        <WorkRow
          row={row}
          scopeKey={scopeKey}
          hostId={hostId}
          now={now}
          expanded={expandedKeys.has(key)}
          selectedSessionId={selectedSessionId}
          onToggle={onToggle}
          onOpen={onOpen}
          onNavigate={onNavigate}
        />
        {expandedKeys.has(key) ? (
          <ChildRows
            row={row}
            scopeKey={scopeKey}
            hostId={hostId}
            now={now}
            selectedSessionId={selectedSessionId}
            page={data.expanded.find(
              (page) => dashboardWorkKey(page.work) === key
            )}
            loading={isRefreshing}
            onPage={(page) => onChildPage(key, page)}
            onOpen={onOpen}
            onNavigate={onNavigate}
          />
        ) : null}
      </Fragment>
    );
  }
  return (
    <div
      className="session-dashboard__scroller"
      ref={scroller}
      onScroll={(event) => onScroll(event.currentTarget.scrollTop)}
      data-testid="dashboard-scroller"
    >
      <table
        className="session-dashboard__table"
        aria-label={t('sessionDashboard.tableLabel')}
      >
        <colgroup>
          <col className="session-dashboard__status-column" />
          <col className="session-dashboard__kind-column" />
          <col className="session-dashboard__work-column" />
          <col className="session-dashboard__directory-column" />
          <col className="session-dashboard__agent-column" />
          <col className="session-dashboard__reply-column" />
          <col className="session-dashboard__actions-column" />
        </colgroup>
        <thead>
          <tr>
            {(
              [
                'status',
                'kind',
                'work',
                'context',
                'agent',
                'reply',
                'actions',
              ] as const
            ).map((column) => (
              <th
                key={column}
                scope="col"
                className={`session-dashboard__${{ status: 'status', kind: 'kind', work: 'work', context: 'directory', agent: 'agent', reply: 'reply', actions: 'actions' }[column]}-column`}
              >
                {t(`sessionDashboard.columns.${column}`)}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.length ? (
            rows
          ) : (
            <tr>
              <td colSpan={7} className="session-dashboard__empty">
                <MessageSquare size={22} aria-hidden="true" />
                <h2>{t('sessionDashboard.emptyTitle')}</h2>
                <p>{t('sessionDashboard.emptyDescription')}</p>
              </td>
            </tr>
          )}
        </tbody>
      </table>
      {isRefreshing ? (
        <span
          className="session-dashboard__refresh-indicator"
          aria-hidden="true"
        >
          <RefreshCw size={11} />
        </span>
      ) : null}
    </div>
  );
}
