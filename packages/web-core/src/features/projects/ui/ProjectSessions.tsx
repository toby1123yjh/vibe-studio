import { useInfiniteQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import type { SessionCursor } from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import { executionDataApi } from '@/shared/lib/executionDataApi';
import { useAppShellProjects } from '@/shared/hooks/useAppShellProjects';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useDeleteExecutionSession } from '@/shared/hooks/useDeleteExecutionSession';
import { DEFAULT_PROJECT_ID } from '@/shared/lib/defaultProject';
import { sessionRoute } from '@/features/app-shell/model/appShell';

interface ProjectSessionsProps {
  projectId: string;
}

export function ProjectSessions({ projectId }: ProjectSessionsProps) {
  const shell = useAppShellProjects();
  return (
    <ProjectSessionsContent
      key={`${shell?.scopeKey}:${projectId}`}
      projectId={projectId}
    />
  );
}

function ProjectSessionsContent({ projectId }: ProjectSessionsProps) {
  const { t } = useTranslation('common');
  const shell = useAppShellProjects();
  const hostId = shell?.hostId ?? null;
  const enabled =
    Boolean(shell) && !(shell?.deployment === 'remote' && !hostId);
  const navigate = useNavigate();
  const query = useInfiniteQuery({
    queryKey: ['project-sessions', shell?.scopeKey, projectId],
    enabled,
    initialPageParam: null as SessionCursor | null,
    queryFn: ({ pageParam, signal }) =>
      executionDataApi.listRecentSessions({
        projectId,
        hostId,
        cursor: pageParam,
        signal,
      }),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    refetchInterval: 10_000,
  });
  const { deleteSession, pendingSessionId } = useDeleteExecutionSession({
    hostId,
    scopeKey: `${shell?.scopeKey}:${projectId}`,
    onDeleted: () => {
      void query.refetch();
    },
  });
  const sessions = Array.from(
    new Map(
      (
        query.data?.pages
          .flatMap((page) => page.sessions)
          .filter((session) => !session.execution_id) ?? []
      ).map((session) => [session.id, session])
    ).values()
  );
  return (
    <div className="space-y-base">
      {!enabled && <p role="alert">{t('defaultProject.offline')}</p>}
      {enabled && query.isPending && <p role="status">{t('states.loading')}</p>}
      {query.isError && (
        <div role="alert">
          <p>{t('defaultProject.loadFailed')}</p>
          <Button
            variant="outline"
            disabled={query.isFetching}
            onClick={() => {
              if (query.isFetchNextPageError) void query.fetchNextPage();
              else void query.refetch();
            }}
          >
            {t('buttons.retry')}
          </Button>
        </div>
      )}
      {enabled &&
        !query.isPending &&
        !query.isError &&
        sessions.length === 0 && (
          <p className="text-low">{t('defaultProject.empty')}</p>
        )}
      {sessions.map((session) => (
        <article
          key={session.id}
          data-session-id={session.id}
          className="flex flex-wrap items-center gap-base rounded-sm border border-border p-base"
        >
          <button
            type="button"
            disabled={!enabled}
            className="min-w-0 flex-1 truncate text-left hover:underline"
            title={session.title}
            onClick={() =>
              void navigate({
                to: `${shell?.deployment === 'remote' ? `/hosts/${encodeURIComponent(hostId ?? '')}` : ''}${sessionRoute(session)}`,
              })
            }
          >
            {session.title}
          </button>
          <Button
            variant="ghost"
            title={t('buttons.delete')}
            disabled={!enabled || pendingSessionId !== null}
            onClick={() =>
              void deleteSession({
                sessionId: session.id,
                workspaceId: session.workspace_id,
                title: session.title,
              })
            }
          >
            {t('buttons.delete')}
          </Button>
        </article>
      ))}
      {query.hasNextPage && (
        <Button
          variant="outline"
          disabled={query.isFetching}
          onClick={() => void query.fetchNextPage()}
        >
          {t('defaultProject.more')}
        </Button>
      )}
    </div>
  );
}

export function DefaultProjectPage() {
  const { t } = useTranslation('common');
  const shell = useAppShellProjects();
  const navigation = useAppNavigation();
  return (
    <section className="h-full overflow-auto p-double">
      <div className="mx-auto max-w-5xl space-y-double">
        <header className="flex items-center justify-between gap-base">
          <h1 className="text-xl font-semibold">{t('defaultProject.name')}</h1>
          <Button
            disabled={Boolean(navigation.agentExecutionUnavailableReason)}
            onClick={() => navigation.goToWorkspacesCreate()}
          >
            {t('appShell.objects.newSession')}
          </Button>
        </header>
        <ProjectSessions
          key={`${shell?.scopeKey}:${DEFAULT_PROJECT_ID}`}
          projectId={DEFAULT_PROJECT_ID}
        />
      </div>
    </section>
  );
}
