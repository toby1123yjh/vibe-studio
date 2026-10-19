import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Check, Copy, ExternalLink, GitBranch, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { DashboardWorkRef } from 'shared/types';
import { createDashboardApi } from '@/shared/lib/dashboardApi';

function CopyValue({ value }: { value: string }) {
  const { t } = useTranslation('common');
  const [copied, setCopied] = useState(false);
  const [failed, setFailed] = useState(false);
  return (
    <span className="session-dashboard__copy-value">
      <code>{value}</code>
      <button
        type="button"
        className="session-dashboard__icon-button"
        title={t('sessionDashboard.context.copy')}
        aria-label={t('sessionDashboard.context.copy')}
        onClick={() => {
          if (!navigator.clipboard) {
            setFailed(true);
            return;
          }
          void navigator.clipboard.writeText(value).then(
            () => {
              setCopied(true);
              setFailed(false);
            },
            () => setFailed(true)
          );
        }}
      >
        {copied ? (
          <Check size={13} aria-hidden="true" />
        ) : (
          <Copy size={13} aria-hidden="true" />
        )}
      </button>
      {failed ? (
        <span role="status">{t('sessionDashboard.context.copyFailed')}</span>
      ) : null}
    </span>
  );
}

export function DashboardContext({
  work,
  scopeKey,
  hostId,
  sessionId,
  agentRunId,
}: {
  work: DashboardWorkRef;
  scopeKey: string;
  hostId: string | null;
  sessionId?: string;
  agentRunId?: string;
}) {
  const { t, i18n } = useTranslation('common');
  const [refresh, setRefresh] = useState(0);
  const context = useQuery({
    queryKey: [
      'session-dashboard-context',
      scopeKey,
      work,
      sessionId,
      agentRunId,
      refresh,
    ],
    queryFn: ({ signal }) =>
      createDashboardApi(hostId).context(
        work,
        { sessionId, agentRunId, refresh: refresh > 0 },
        signal
      ),
    staleTime: 30_000,
    retry: false,
    refetchOnWindowFocus: false,
  });
  return (
    <section
      className="session-dashboard__context"
      aria-label={t('sessionDashboard.context.title')}
      data-testid="dashboard-context"
    >
      <header>
        <h3>{t('sessionDashboard.context.title')}</h3>
        <button
          type="button"
          className="session-dashboard__text-button"
          disabled={context.isFetching}
          onClick={() => setRefresh((value) => value + 1)}
        >
          <RefreshCw size={13} aria-hidden="true" />
          {t('sessionDashboard.context.refresh')}
        </button>
      </header>
      {context.isPending ? (
        <p role="status">{t('sessionDashboard.loading')}</p>
      ) : null}
      {context.isError ? (
        <p role="alert">
          {t('sessionDashboard.context.failed')}{' '}
          <button
            type="button"
            className="session-dashboard__text-button"
            onClick={() => void context.refetch()}
          >
            {t('buttons.retry')}
          </button>
        </p>
      ) : null}
      {context.data ? (
        <>
          {context.data.sessions.map((session) => (
            <dl
              key={`${session.session_id}:${session.agent_run_id}`}
              className="session-dashboard__context-fields"
            >
              <div>
                <dt>{t('sessionDashboard.context.session')}</dt>
                <dd>
                  <CopyValue value={session.session_id} />
                </dd>
              </div>
              <div>
                <dt>{t('sessionDashboard.context.directory')}</dt>
                <dd>
                  {session.directory.path ? (
                    <CopyValue value={session.directory.path} />
                  ) : (
                    t('sessionDashboard.unavailable')
                  )}
                </dd>
              </div>
              <div>
                <dt>{t('sessionDashboard.context.mode')}</dt>
                <dd>
                  {t(`sessionDashboard.directory.${session.directory.kind}`, {
                    defaultValue: session.directory.kind,
                  })}
                </dd>
              </div>
              {session.agent_run_id ? (
                <div>
                  <dt>{t('sessionDashboard.context.run')}</dt>
                  <dd>
                    <CopyValue value={session.agent_run_id} />
                  </dd>
                </div>
              ) : null}
              {session.agent ? (
                <div>
                  <dt>{t('sessionDashboard.context.configuration')}</dt>
                  <dd>
                    {[
                      session.agent.runtime_profile_id,
                      session.agent.model_id,
                      session.agent.reasoning_id,
                    ]
                      .filter(Boolean)
                      .join(' · ') || t('sessionDashboard.unavailable')}{' '}
                    <span className="session-dashboard__muted">
                      (
                      {t(
                        `sessionDashboard.configuration.${session.agent.configuration_source}`,
                        { defaultValue: session.agent.configuration_source }
                      )}
                      )
                    </span>
                  </dd>
                </div>
              ) : null}
              {session.started_at ? (
                <div>
                  <dt>{t('sessionDashboard.context.started')}</dt>
                  <dd>
                    {new Date(session.started_at).toLocaleString(i18n.language)}
                  </dd>
                </div>
              ) : null}
              {session.finished_at ? (
                <div>
                  <dt>{t('sessionDashboard.context.finished')}</dt>
                  <dd>
                    {new Date(session.finished_at).toLocaleString(
                      i18n.language
                    )}
                  </dd>
                </div>
              ) : null}
            </dl>
          ))}
          {context.data.repositories.map((repo) => (
            <div key={repo.path} className="session-dashboard__repository">
              <div>
                <GitBranch size={14} aria-hidden="true" />
                <CopyValue value={repo.path} />
              </div>
              <p>
                {t(`sessionDashboard.git.${repo.status}`, {
                  defaultValue: repo.status,
                })}
                {repo.branch
                  ? ` · ${repo.branch}`
                  : repo.commit
                    ? ` · ${repo.commit.slice(0, 8)}`
                    : ''}
              </p>
              {repo.issue ? (
                <p className="session-dashboard__issue">{repo.issue}</p>
              ) : null}
              {repo.remotes.length ? (
                repo.remotes.map((remote) => (
                  <div key={`${remote.name}:${remote.url}`}>
                    <span>{remote.name}</span>
                    <CopyValue value={remote.url} />
                    {remote.web_url && /^https?:\/\//i.test(remote.web_url) ? (
                      <a
                        href={remote.web_url}
                        target="_blank"
                        rel="noreferrer"
                        aria-label={t(
                          'sessionDashboard.context.openRepository'
                        )}
                      >
                        <ExternalLink size={14} aria-hidden="true" />
                      </a>
                    ) : null}
                  </div>
                ))
              ) : repo.status === 'git' ? (
                <p className="session-dashboard__muted">
                  {t('sessionDashboard.git.noRemote')}
                </p>
              ) : null}
            </div>
          ))}
          {!context.data.sessions.length &&
          !context.data.repositories.length ? (
            <p>{t('sessionDashboard.context.empty')}</p>
          ) : null}
          {context.data.issues.map((issue, index) => (
            <p
              key={`${issue.code}:${index}`}
              className="session-dashboard__issue"
            >
              {issue.message}
            </p>
          ))}
        </>
      ) : null}
    </section>
  );
}
