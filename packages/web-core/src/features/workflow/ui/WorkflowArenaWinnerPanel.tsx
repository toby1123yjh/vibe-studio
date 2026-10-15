import { useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useQueryClient } from '@tanstack/react-query';
import {
  AlertCircle,
  CheckCircle,
  ExternalLink,
  GitBranch,
  Swords,
} from 'lucide-react';
import { Button } from '@vibe/ui/components/Button';
import {
  DegradedState,
  EmptyState,
  ErrorState,
  LoadingState,
} from '@vibe/ui/components/StateSurface';
import {
  arenaQueryKeys,
  useArenaGroup,
  useArenaInvalidators,
} from '@/shared/hooks/useArenaGroup';
import type { ArenaGroupResponse } from '@/shared/lib/arenaApi';
import { useWorkflowRunMutations } from '@/shared/hooks/useWorkflowRun';
import { cn } from '@/shared/lib/utils';
import { ArenaWinnerConfirmDialog } from '@/shared/dialogs/arena/ArenaWinnerConfirmDialog';
import { useHostId } from '@/shared/providers/HostIdProvider';
import {
  buildArenaComparisonView,
  type ArenaComparisonCandidate,
} from '../../arena/model/arenaComparisonView';
import { buildWorkflowConversationHref } from '../model/workflowConversationTarget';

export interface WorkflowArenaWinnerPanelProps {
  arenaGroupId: string | null;
  className?: string;
  taskId: string;
  nodeId: string;
  nodeExecutionId: string;
  projectId: string;
  runId: string;
  onOpenCandidate?: (candidate: ArenaComparisonCandidate) => void;
  allowWinnerSelection?: boolean;
}

export function WorkflowArenaWinnerPanel(props: WorkflowArenaWinnerPanelProps) {
  const { arenaGroupId, nodeId, nodeExecutionId, runId } = props;
  return (
    <WorkflowArenaWinnerPanelForIdentity
      key={`${runId}:${nodeId}:${nodeExecutionId}:${arenaGroupId ?? 'none'}`}
      {...props}
    />
  );
}

function WorkflowArenaWinnerPanelForIdentity({
  arenaGroupId,
  className,
  taskId,
  nodeId,
  nodeExecutionId,
  projectId,
  runId,
  onOpenCandidate,
  allowWinnerSelection = true,
}: WorkflowArenaWinnerPanelProps) {
  const { t } = useTranslation('common');
  const hostId = useHostId();
  const queryClient = useQueryClient();
  const { selectArenaWinner, isSelectingArenaWinner } =
    useWorkflowRunMutations();
  const { invalidateGroup } = useArenaInvalidators();
  const [confirmationCandidate, setConfirmationCandidate] =
    useState<ArenaComparisonCandidate | null>(null);
  const [winnerDialogOpen, setWinnerDialogOpen] = useState(false);
  const [winnerTrigger, setWinnerTrigger] = useState<HTMLElement | null>(null);
  const [confirmationError, setConfirmationError] = useState<string | null>(
    null
  );
  const winnerMutationLockRef = useRef(false);
  const {
    data: arenaGroup,
    error: arenaError,
    isFetching,
    isPending,
    refetch,
  } = useArenaGroup(arenaGroupId, {
    enabled: !!arenaGroupId,
  });

  const comparison = useMemo(
    () =>
      arenaGroup
        ? buildArenaComparisonView(arenaGroup, {
            attempt: (order) =>
              t('arena.workspace.attemptName', { index: order }),
            synthesis: (order) => `${t('arena.purpose.synthesis')} ${order}`,
          })
        : null,
    [arenaGroup, t]
  );
  const candidates = comparison?.candidates ?? [];
  const getArenaLoadFailure = (error: unknown) =>
    t('workflow.arenaWinner.loadFailed', {
      message:
        error instanceof Error
          ? error.message
          : error
            ? String(error)
            : t('workflow.arenaWinner.unknownError'),
    });
  const arenaLoadFailure = getArenaLoadFailure(arenaError);
  const [arenaRetryPending, setArenaRetryPending] = useState(false);
  const arenaRetryLockRef = useRef(false);
  const handleRetryArenaGroup = async () => {
    if (arenaRetryLockRef.current || isFetching) return;
    arenaRetryLockRef.current = true;
    setArenaRetryPending(true);
    try {
      await refetch();
    } finally {
      arenaRetryLockRef.current = false;
      setArenaRetryPending(false);
    }
  };
  const retryArenaGroupAction = (
    <Button
      type="button"
      variant="outline"
      className="min-h-11"
      loading={arenaRetryPending || isFetching}
      loadingLabel={t('arena.actions.retrying')}
      onClick={() => void handleRetryArenaGroup()}
    >
      {t('buttons.retry')}
    </Button>
  );

  const arenaHref = arenaGroupId
    ? `${hostId ? `/hosts/${encodeURIComponent(hostId)}` : ''}/projects/${projectId}/tasks/${taskId}/arena/${arenaGroupId}`
    : null;

  const handleConfirmWinner = async () => {
    if (
      !confirmationCandidate ||
      winnerMutationLockRef.current ||
      !allowWinnerSelection
    )
      return;
    winnerMutationLockRef.current = true;
    try {
      if (!arenaGroupId) {
        setConfirmationError(t('workflow.arenaWinner.selectFailed'));
        return;
      }
      const currentState = queryClient.getQueryState<ArenaGroupResponse>(
        arenaQueryKeys.group(arenaGroupId)
      );
      if (currentState?.error) {
        setConfirmationError(getArenaLoadFailure(currentState.error));
        return;
      }
      const currentComparison = currentState?.data
        ? buildArenaComparisonView(currentState.data, {
            attempt: (order) =>
              t('arena.workspace.attemptName', { index: order }),
            synthesis: (order) => `${t('arena.purpose.synthesis')} ${order}`,
          })
        : null;
      const currentCandidate = currentComparison?.candidates.find(
        ({ candidateId }) => candidateId === confirmationCandidate.candidateId
      );
      if (!currentCandidate?.canSelectWinner) {
        setConfirmationError(t('workflow.arenaWinner.selectFailed'));
        return;
      }
      setConfirmationError(null);
      await selectArenaWinner({
        runId,
        nodeId,
        payload: {
          candidate_id: currentCandidate.candidateId,
          node_execution_id: nodeExecutionId,
        },
      });
      if (arenaGroupId) {
        invalidateGroup(arenaGroupId);
      }
      setWinnerDialogOpen(false);
    } catch (err) {
      setConfirmationError(
        err instanceof Error
          ? err.message
          : t('workflow.arenaWinner.selectFailed')
      );
    } finally {
      winnerMutationLockRef.current = false;
    }
  };

  return (
    <div
      className={cn(
        'space-y-half rounded border border-warning/50 bg-warning/10 p-half',
        className
      )}
    >
      <div className="flex items-start justify-between gap-half">
        <div className="min-w-0">
          <h4 className="flex items-center gap-half text-sm font-semibold text-warning">
            <Swords className="h-4 w-4" />
            {t('workflow.arenaWinner.title')}
          </h4>
          <p className="mt-1 text-xs text-high">
            {t('workflow.arenaWinner.description')}
          </p>
        </div>
        {arenaHref ? (
          <a
            className="inline-flex min-h-8 shrink-0 items-center gap-half rounded border border-secondary bg-panel px-half py-1 text-xs font-medium text-brand hover:bg-secondary"
            href={arenaHref}
          >
            <ExternalLink className="h-3.5 w-3.5" />
            {t('arena.title')}
          </a>
        ) : null}
      </div>

      {!arenaGroupId ? (
        <p className="text-xs text-low">
          {t('workflow.arenaWinner.noGroupLink')}
        </p>
      ) : isPending && !arenaGroup ? (
        <LoadingState
          compact
          title={t('workflow.arenaWinner.loadingAttempts')}
        />
      ) : !arenaGroup ? (
        <ErrorState
          compact
          title={arenaLoadFailure}
          action={retryArenaGroupAction}
        />
      ) : (
        <div className="space-y-half">
          {arenaError ? (
            <DegradedState
              compact
              role="status"
              aria-live="polite"
              aria-atomic="true"
              title={arenaLoadFailure}
              action={retryArenaGroupAction}
            />
          ) : null}
          {candidates.length === 0 ? (
            <EmptyState compact title={t('workflow.arenaWinner.noAttempts')} />
          ) : null}
          {candidates.map((candidate) => {
            const workspaceHref = buildWorkflowConversationHref({
              hostId,
              projectId,
              taskId,
              workspaceId: candidate.workspaceId,
              sessionId: candidate.sessionId,
            });
            const isApplying =
              isSelectingArenaWinner &&
              confirmationCandidate?.candidateId === candidate.candidateId;

            return (
              <div
                key={candidate.candidateId}
                className={cn(
                  'rounded border bg-panel p-half text-xs',
                  candidate.isWinner ? 'border-success/60' : 'border-secondary'
                )}
              >
                <div className="flex items-start justify-between gap-half">
                  <div className="min-w-0 space-y-1">
                    <div className="flex items-center gap-half">
                      {candidate.isWinner ? (
                        <CheckCircle className="h-3.5 w-3.5 text-success" />
                      ) : candidate.canSelectWinner ? (
                        <Swords className="h-3.5 w-3.5 text-warning" />
                      ) : (
                        <AlertCircle className="h-3.5 w-3.5 text-low" />
                      )}
                      <span className="truncate font-medium text-high">
                        {candidate.label}
                      </span>
                    </div>
                    <div className="flex flex-wrap items-center gap-x-half gap-y-1 text-low">
                      <span>
                        {candidate.executorLabel ||
                          t('workflow.arenaWinner.unknownExecutor')}
                      </span>
                      <span>
                        {t(
                          `arena.agentStatus.${candidate.agentRunStatus ?? 'not_started'}`
                        )}
                      </span>
                      <span>{t(`arena.status.${candidate.arenaStatus}`)}</span>
                      <span>
                        {candidate.workspace.has_uncommitted_changes === true
                          ? t('workflow.arenaWinner.changes')
                          : candidate.workspace.has_uncommitted_changes ===
                              false
                            ? t('workflow.arenaWinner.noChanges')
                            : t('workflow.arenaWinner.changesUnknown')}
                      </span>
                    </div>
                    <div className="flex min-w-0 items-center gap-half text-low">
                      <GitBranch className="h-3 w-3 shrink-0" />
                      <span className="truncate font-mono">
                        {candidate.branch}
                      </span>
                    </div>
                  </div>

                  <div className="flex shrink-0 items-center gap-half">
                    {onOpenCandidate ? (
                      <Button
                        type="button"
                        variant="outline"
                        size="xs"
                        onClick={() => onOpenCandidate(candidate)}
                        aria-label={t('workflow.arenaWinner.openWorkspace', {
                          label: candidate.label,
                        })}
                      >
                        <ExternalLink
                          className="h-3.5 w-3.5"
                          aria-hidden="true"
                        />
                      </Button>
                    ) : workspaceHref ? (
                      <a
                        className="inline-flex min-h-8 items-center rounded border border-secondary px-half py-1 text-xs font-medium text-brand hover:bg-secondary"
                        href={workspaceHref}
                        aria-label={t('workflow.arenaWinner.openWorkspace', {
                          label: candidate.label,
                        })}
                      >
                        <ExternalLink className="h-3.5 w-3.5" />
                      </a>
                    ) : null}
                    <Button
                      type="button"
                      size="xs"
                      disabled={
                        !candidate.canSelectWinner ||
                        !allowWinnerSelection ||
                        isSelectingArenaWinner ||
                        Boolean(arenaError)
                      }
                      onClick={(event) => {
                        setConfirmationCandidate(candidate);
                        setWinnerTrigger(event.currentTarget);
                        setConfirmationError(null);
                        setWinnerDialogOpen(true);
                      }}
                    >
                      {candidate.isWinner
                        ? t('workflow.arenaWinner.selected')
                        : isApplying
                          ? t('workflow.arenaWinner.applying')
                          : t('workflow.arenaWinner.selectWinner')}
                    </Button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}

      <ArenaWinnerConfirmDialog
        candidate={confirmationCandidate}
        archiveSiblingCount={Math.max(0, candidates.length - 1)}
        outcome="workflow"
        open={winnerDialogOpen}
        isPending={isSelectingArenaWinner}
        error={confirmationError}
        returnFocusTarget={winnerTrigger}
        onOpenChange={(open) => {
          setWinnerDialogOpen(open);
          if (!open) setConfirmationError(null);
        }}
        onConfirm={handleConfirmWinner}
      />
    </div>
  );
}
