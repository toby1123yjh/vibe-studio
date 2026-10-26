import { type ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { Button } from '@vibe/ui/components/Button';
import { ErrorState, LoadingState } from '@vibe/ui/components/StateSurface';
import { sessionsApi } from '@/shared/lib/api';
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
  const isLoading =
    workspaceQuery.isLoading || reposQuery.isLoading || sessionsQuery.isLoading;
  const error = workspaceQuery.error ?? reposQuery.error ?? sessionsQuery.error;

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
  if (error || workspaceQuery.data?.id !== workspaceId || !session) {
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
          <Button
            onClick={() => {
              void workspaceQuery.refetch();
              void reposQuery.refetch();
              void sessionsQuery.refetch();
            }}
          >
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
          selectedWorkspace={workspaceQuery.data}
          selectedSession={session}
          selectedSessionId={sessionId}
          sessions={[session]}
          repos={reposQuery.repos}
          isLoading={false}
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
