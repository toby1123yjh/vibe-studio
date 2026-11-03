import { useCallback, useMemo, type ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { Button } from '@vibe/ui/components/Button';
import { ErrorState, LoadingState } from '@vibe/ui/components/StateSurface';
import { ApiError, sessionsApi } from '@/shared/lib/api';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { ExecutionProcessesProvider } from '@/shared/providers/ExecutionProcessesProvider';
import { useWorkspaceRecord } from '@/shared/hooks/useWorkspaceRecord';
import { useWorkspaceRepo } from '@/shared/hooks/useWorkspaceRepo';
import { workspaceSessionKeys } from '@/shared/hooks/workspaceSessionKeys';
import { WorkspaceFilePreviewActionsProvider } from '@/features/workspace-files/model/WorkspaceFilePreviewActionsProvider';
import type { WorkspaceFilePreviewTarget } from '@/features/workspace-files/model/types';
import {
  conversationScopeKey,
  type WorkspaceConversationViewState,
} from '../model/conversationViewState';
import { WorkspaceConversationContent } from './WorkspaceConversationContent';

export interface WorkspaceSessionConversationProps {
  workspaceId: string;
  sessionId: string;
  readOnly?: boolean;
  beforeConversation?: ReactNode;
  /** Replaces the visible chat area without destroying its draft or scroll state. */
  previewContent?: ReactNode;
  onOpenWorkspaceFilePreview?: (target: WorkspaceFilePreviewTarget) => void;
  viewState?: WorkspaceConversationViewState;
  onViewStateChange?: (state: WorkspaceConversationViewState) => void;
}

const ignoreSessionSelection = () => {};

/** Loads only the explicitly bound Session. Never creates or selects a sibling. */
function BoundWorkspaceConversation({
  workspaceId,
  sessionId,
  onOpenWorkspaceFilePreview,
  ...presentation
}: WorkspaceSessionConversationProps) {
  const { t } = useTranslation('common');
  const hostId = useHostId();
  const workspaceQuery = useWorkspaceRecord(workspaceId);
  const reposQuery = useWorkspaceRepo(workspaceId);
  const sessionsQuery = useQuery({
    queryKey: workspaceSessionKeys.byWorkspace(workspaceId, hostId),
    queryFn: () => sessionsApi.getByWorkspace(workspaceId, hostId),
    enabled: Boolean(workspaceId && sessionId),
  });
  const session = sessionsQuery.data?.find(
    (item) => item.id === sessionId && item.workspace_id === workspaceId
  );
  const workspace =
    workspaceQuery.data?.id === workspaceId ? workspaceQuery.data : undefined;
  const sessions = useMemo(() => (session ? [session] : []), [session]);
  const { refetch: refetchWorkspace } = workspaceQuery;
  const { refetch: refetchSessions } = sessionsQuery;
  const retryBinding = useCallback(async () => {
    await Promise.all([refetchWorkspace(), refetchSessions()]);
  }, [refetchWorkspace, refetchSessions]);
  // Repository reads feed the independently scoped Changes/tools state. They
  // must not hold up the transcript or replace its editor on failure.
  const isLoading =
    (!workspace && workspaceQuery.isLoading) ||
    (!session && sessionsQuery.isLoading);
  const bindingError = [workspaceQuery.error, sessionsQuery.error].find(
    (failure) =>
      failure instanceof ApiError &&
      [401, 403, 404, 410].includes(failure.statusCode ?? 0)
  );
  const error = bindingError ?? workspaceQuery.error ?? sessionsQuery.error;

  if (isLoading) {
    return (
      <LoadingState
        className="min-h-0 flex-1"
        title={t('workflow.conversation.loading', {
          defaultValue: 'Loading conversation…',
        })}
      />
    );
  }
  if (bindingError || !workspace || !session) {
    return (
      <ErrorState
        className="min-h-0 flex-1"
        title={t('workflow.conversation.unavailable', {
          defaultValue: 'Conversation is unavailable',
        })}
        description={
          error instanceof Error
            ? error.message
            : t('workflow.conversation.missingBinding', {
                defaultValue:
                  'The bound session was not found in this workspace.',
              })
        }
        action={
          <Button onClick={() => void retryBinding()}>
            {t('buttons.retry', { defaultValue: 'Retry' })}
          </Button>
        }
      />
    );
  }

  return (
    <ExecutionProcessesProvider sessionId={sessionId}>
      <WorkspaceFilePreviewActionsProvider
        enabled={Boolean(onOpenWorkspaceFilePreview)}
        onOpenWorkspaceFilePreview={onOpenWorkspaceFilePreview ?? (() => {})}
      >
        <WorkspaceConversationContent
          {...presentation}
          selectedWorkspace={workspace}
          selectedSession={session}
          selectedSessionId={sessionId}
          sessions={sessions}
          repos={reposQuery.repos}
          isLoading={false}
          workspaceState={error ? 'degraded' : 'ready'}
          isRetryingWorkspace={
            workspaceQuery.isFetching || sessionsQuery.isFetching
          }
          onRetryWorkspace={retryBinding}
          isNewSessionMode={false}
          onSelectSession={ignoreSessionSelection}
          onStartNewSession={ignoreSessionSelection}
          presentation="embedded"
          showSessionControls={false}
        />
      </WorkspaceFilePreviewActionsProvider>
    </ExecutionProcessesProvider>
  );
}

export function WorkspaceSessionConversation(
  props: WorkspaceSessionConversationProps
) {
  const hostId = useHostId();
  return (
    <BoundWorkspaceConversation
      key={conversationScopeKey(hostId, props.workspaceId, props.sessionId)}
      {...props}
    />
  );
}
