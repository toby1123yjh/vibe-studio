import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '@vibe/ui/components/Button';
import {
  DegradedState,
  ErrorState,
  LoadingState,
} from '@vibe/ui/components/StateSurface';
import { WorkspaceContextHeader } from '@/shared/components/WorkspaceContextHeader';
import {
  useWorkflowRun,
  useWorkflowRunMutations,
} from '@/shared/hooks/useWorkflowRun';
import { useWorkflowRunEvents } from '@/shared/hooks/useWorkflowRunEvents';
import { useWorkflowTemplate } from '@/shared/hooks/useWorkflowTemplates';
import { cn } from '@/shared/lib/utils';
import { buildWorkflowRunDashboardSummary } from '../model/workflowRunView';
import {
  getWorkflowRunActionGate,
  getWorkflowRuntimeView,
} from '../model/workflowRuntimeView';
import { workflowNodeStatusKey } from './workflowI18n';
import { WorkflowRunCanvasTab } from './WorkflowRunCanvasTab';
import { WorkflowRunResult } from './WorkflowRunResult';
import { WorkflowMainSessionButton } from './WorkflowMainSessionButton';
import { useHostId } from '@/shared/providers/HostIdProvider';
import type { WorkflowMainSessionView } from 'shared/types';

export interface WorkflowRunPageProps {
  projectId: string;
  runId: string;
}

type CancelSubmissionState = 'idle' | 'submitting' | 'awaiting-projection';

export function WorkflowRunPage({ projectId, runId }: WorkflowRunPageProps) {
  const { t } = useTranslation('common');
  const hostId = useHostId();
  const [mainSessionRequest, setMainSessionRequest] = useState<{
    scope: string;
    prepared: WorkflowMainSessionView;
    trigger: HTMLElement;
  } | null>(null);
  const {
    data: run,
    isLoading,
    isFetching,
    error,
    refetch,
  } = useWorkflowRun(runId);
  const { data: template } = useWorkflowTemplate(run?.workflow_id, {
    enabled: Boolean(run?.workflow_id),
  });
  const { cancelRun, isCanceling } = useWorkflowRunMutations();
  const cancelLockRef = useRef(false);
  const [cancelState, setCancelState] = useState<CancelSubmissionState>('idle');
  const [cancelError, setCancelError] = useState<string | null>(null);
  const shouldStreamEvents =
    !!run &&
    (run.status === 'pending' ||
      run.status === 'running' ||
      run.status === 'awaiting_human' ||
      run.status === 'awaiting_arena' ||
      run.status === 'cancelling');

  useWorkflowRunEvents(runId, {
    enabled: shouldStreamEvents,
  });

  if (isLoading) {
    return (
      <LoadingState
        className="h-full w-full bg-primary"
        title={t('workflow.runPage.loadingRun')}
      />
    );
  }

  if (!run) {
    return (
      <ErrorState
        className="h-full w-full bg-primary"
        title={t('workflow.runPage.loadFailed', { runId })}
        description={error instanceof Error ? error.message : undefined}
        action={
          <Button
            type="button"
            variant="outline"
            loading={isFetching}
            onClick={() => void refetch()}
          >
            {t('buttons.retry')}
          </Button>
        }
      />
    );
  }

  const runtimeView = getWorkflowRuntimeView(run);
  const runActionGate = getWorkflowRunActionGate(run);
  const cancelPending =
    runActionGate.cancellationPending ||
    (runActionGate.canCancel && (isCanceling || cancelState !== 'idle'));
  const showCancel =
    runActionGate.canCancel || runActionGate.cancellationPending;
  const summary = buildWorkflowRunDashboardSummary(run, runtimeView);
  const statusTone =
    run.status === 'succeeded'
      ? 'text-success'
      : run.status === 'failed' || run.status === 'canceled'
        ? 'text-error'
        : run.status === 'awaiting_human' || run.status === 'awaiting_arena'
          ? 'text-warning'
          : 'text-high';

  const handleCancel = async () => {
    if (cancelLockRef.current || cancelState !== 'idle') return;
    if (!getWorkflowRunActionGate(run).canCancel) return;

    cancelLockRef.current = true;
    setCancelState('submitting');
    setCancelError(null);
    try {
      await cancelRun(run.id);
      setCancelState('awaiting-projection');
    } catch (cancelRunError) {
      setCancelState('idle');
      setCancelError(
        cancelRunError instanceof Error
          ? cancelRunError.message
          : t('workflow.dashboard.cancelFailed')
      );
    } finally {
      cancelLockRef.current = false;
    }
  };

  return (
    <div className="flex h-full flex-col bg-primary">
      <header className="flex flex-none flex-col gap-half border-b border-secondary bg-panel px-base py-half sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 flex-col gap-half">
          <div className="flex min-w-0 flex-wrap items-center gap-base">
            <h1 className="max-w-[320px] truncate text-sm font-medium text-high sm:max-w-lg">
              {template?.name ||
                t('workflow.runPage.workflowRun', {
                  id: run.id.slice(0, 8),
                })}
            </h1>
            <span
              className={cn(
                'rounded-full bg-secondary px-2 py-0.5 text-xs font-medium',
                statusTone
              )}
              aria-live="polite"
            >
              {run.status === 'pending' &&
              (run.queue_phase === 'queued' || run.queue_phase === 'starting')
                ? t(`externalIntegrations.${run.queue_phase}`, {
                    ns: 'settings',
                  })
                : t(`workflow.runStatus.${workflowNodeStatusKey(run.status)}`)}
            </span>
            {run.delivery?.completeness === 'partial' ? (
              <span
                className="rounded bg-warning/10 px-2 py-0.5 text-xs font-medium text-warning"
                data-testid="workflow-partial-delivery"
              >
                {t('workflow.split.partialDelivery')}
              </span>
            ) : null}
            <span className="text-xs text-low">
              {t('workflow.runPage.progress', {
                completed: summary.completedSteps,
                total: summary.freshSteps,
              })}
            </span>
            {runtimeView.reused_node_count > 0 ? (
              <span className="text-xs text-brand">
                {t('workflow.dashboard.reusedCount', {
                  count: runtimeView.reused_node_count,
                })}
              </span>
            ) : null}
            {runtimeView.skipped_node_count > 0 ? (
              <span className="text-xs text-low">
                {t('workflow.dashboard.skippedCount', {
                  count: runtimeView.skipped_node_count,
                })}
              </span>
            ) : null}
          </div>
          <WorkspaceContextHeader
            workspaceId={run.workspace_id}
            className="max-w-[720px]"
          />
          {cancelError ? (
            <p className="text-xs text-error" role="alert">
              {cancelError}
            </p>
          ) : null}
        </div>

        <div className="flex items-center gap-2">
          <WorkflowMainSessionButton
            key={`${hostId}:${run.id}`}
            projectId={projectId}
            workflowId={run.workflow_id}
            taskId={run.task_id}
            onPrepared={(prepared, trigger) =>
              setMainSessionRequest({
                scope: `${hostId}:${run.id}`,
                prepared,
                trigger,
              })
            }
          />
          <WorkflowRunResult key={run.id} run={run} projectId={projectId} />
          {showCancel ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={cancelPending || !runActionGate.canCancel}
              aria-busy={cancelPending}
              onClick={() => void handleCancel()}
            >
              {cancelPending
                ? t('workflow.runPage.cancelling')
                : t('workflow.dashboard.cancelRun')}
            </Button>
          ) : null}
        </div>
      </header>

      <div className="relative min-h-0 flex-1">
        {error ? (
          <DegradedState
            compact
            className="absolute left-1/2 top-4 z-20 w-[min(28rem,calc(100%-2rem))] -translate-x-1/2 border border-warning/30 shadow-md"
            title={t('workflow.runPage.loadFailed', { runId })}
            description={error instanceof Error ? error.message : undefined}
            action={
              <Button
                type="button"
                variant="outline"
                size="sm"
                loading={isFetching}
                onClick={() => void refetch()}
              >
                {t('buttons.retry')}
              </Button>
            }
          />
        ) : null}

        <WorkflowRunCanvasTab
          key={`${hostId}:${run.id}`}
          projectId={projectId}
          run={run}
          mainSessionRequest={
            mainSessionRequest?.scope === `${hostId}:${run.id}`
              ? mainSessionRequest.prepared
              : null
          }
          mainSessionTrigger={
            mainSessionRequest?.scope === `${hostId}:${run.id}`
              ? mainSessionRequest.trigger
              : null
          }
        />
      </div>
    </div>
  );
}
