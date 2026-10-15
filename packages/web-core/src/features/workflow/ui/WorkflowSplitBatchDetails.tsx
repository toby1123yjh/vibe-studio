import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ExternalLink } from 'lucide-react';
import type {
  WorkflowRunResponse,
  WorkflowSplitBatchView,
  WorkflowSubmission,
} from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { workflowManagementApi } from '@/shared/lib/workflowManagementApi';
import { useHostId } from '@/shared/providers/HostIdProvider';
import type { WorkflowGraph } from '../model/workflowGraph';
import {
  buildWorkflowPartialSummarySubmission,
  canRequestWorkflowPartialSummary,
  getWorkflowSplitProgress,
  getWorkflowSplitStages,
} from '../model/workflowRuntimeView';
import { workflowSplitStateKey } from './workflowI18n';
import {
  WorkflowFileReferences,
  type WorkflowFilePreviewTarget,
} from './WorkflowFileReferences';
import { buildWorkflowConversationHref } from '../model/workflowConversationTarget';
import type { WorkflowSplitSubtaskView } from 'shared/types';

export function WorkflowSplitBatchDetails({
  batch,
  graph,
  projectId,
  run,
  onOpenSubtask,
  onOpenMainSession,
  onOpenFile,
}: {
  batch: WorkflowSplitBatchView;
  graph: WorkflowGraph;
  projectId: string;
  run: WorkflowRunResponse;
  workspaceHref: string | null;
  onOpenSubtask?: (task: WorkflowSplitSubtaskView) => void;
  onOpenMainSession?: () => void;
  onOpenFile?: (target: WorkflowFilePreviewTarget) => void;
}) {
  const { t } = useTranslation('common');
  const hostId = useHostId();
  const progress = getWorkflowSplitProgress(batch.subtasks);
  const stages = getWorkflowSplitStages(batch);
  const mainSessionHref = buildWorkflowConversationHref({
    hostId,
    projectId,
    taskId: run.task_id,
    workspaceId: run.workspace_id,
    sessionId: batch.handover?.session_id ?? null,
  });
  const getNodeLabel = (id: string) =>
    graph.nodes.find((node) => node.id === id)?.data.display_name || id;

  return (
    <section
      className="space-y-base rounded border border-secondary p-base"
      data-testid="workflow-split-batch"
    >
      <div className="flex flex-wrap items-center justify-between gap-half">
        <h3 className="text-xs font-semibold text-high">
          {t('workflow.split.subtasks')}
        </h3>
        <span className="text-xs text-low">
          {t('workflow.split.states.' + workflowSplitStateKey(batch.state))}
        </span>
      </div>
      <p className="text-xs text-normal" data-testid="workflow-split-progress">
        {t('workflow.split.progress', { ...progress })}
        {progress.failed > 0 ? (
          <span className="ml-half text-error">
            {t('workflow.split.failedCount', { count: progress.failed })}
          </span>
        ) : null}
      </p>
      {batch.error_text ? (
        <p className="break-words text-xs text-error" role="alert">
          {batch.error_text}
        </p>
      ) : null}
      <div className="space-y-half">
        <h4 className="text-xs font-medium text-low">
          {t('workflow.split.controlFiles')}
        </h4>
        <WorkflowFileReferences
          runId={run.id}
          onOpenFile={onOpenFile}
          files={[
            { path: batch.manifest_path, sha256: batch.manifest_sha256 },
            ...(batch.dispatch_path ? [{ path: batch.dispatch_path }] : []),
          ]}
        />
      </div>
      {!batch.dispatch_path && batch.state === 'pending' ? (
        <p
          className="rounded bg-secondary p-half text-xs text-warning"
          role="status"
        >
          {t('workflow.split.waitingDispatch')}
        </p>
      ) : null}
      {batch.handover ? (
        <div className="space-y-half border-t border-secondary pt-base">
          <p className="flex flex-wrap items-center justify-between gap-half text-xs">
            <span className="font-medium text-high">
              {t('workflow.split.mainHandover')}
            </span>
            <span className="text-low">
              {t(
                'workflow.split.states.' +
                  workflowSplitStateKey(batch.handover.state)
              )}
            </span>
          </p>
          {batch.handover.error_text ? (
            <p className="break-words text-xs text-error" role="alert">
              {batch.handover.error_text}
            </p>
          ) : null}
          {onOpenMainSession ? (
            <Button
              type="button"
              variant="ghost"
              size="xs"
              onClick={onOpenMainSession}
            >
              {t('workflow.split.openMainSession')}
            </Button>
          ) : mainSessionHref ? (
            <a
              href={mainSessionHref}
              className="inline-flex items-center gap-half rounded text-xs text-brand focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
            >
              {t('workflow.split.openMainSession')}
              <ExternalLink className="h-3 w-3" aria-hidden="true" />
            </a>
          ) : null}
        </div>
      ) : null}
      {batch.dispatch_path ? (
        <p className="text-xs leading-relaxed text-low">
          {t('workflow.split.stagesHint')}
        </p>
      ) : null}
      <div className="space-y-base">
        {stages.map((stage) => (
          <div
            key={stage.index}
            className="space-y-half"
            data-testid="workflow-split-stage"
          >
            <h4 className="text-xs font-medium text-low">
              {batch.dispatch_path
                ? t('workflow.split.stage', { index: stage.index + 1 })
                : t('workflow.split.pendingAssignments')}
            </h4>
            <ul className="divide-y divide-secondary rounded border border-secondary bg-primary">
              {stage.subtasks.map((task) => {
                const sessionHref = buildWorkflowConversationHref({
                  hostId,
                  projectId,
                  taskId: run.task_id,
                  workspaceId: run.workspace_id,
                  sessionId: task.session_id,
                });
                const handoff = run.handoffs.find(
                  (item) => item.node_execution_id === task.node_execution_id
                );
                return (
                  <li
                    key={task.id}
                    className="space-y-half p-base"
                    data-testid="workflow-split-subtask"
                  >
                    <div className="flex items-start justify-between gap-half">
                      <span className="min-w-0 break-words text-xs font-medium text-high">
                        {task.title}
                      </span>
                      <span
                        className={
                          task.state === 'failed'
                            ? 'shrink-0 text-xs text-error'
                            : 'shrink-0 text-xs text-low'
                        }
                      >
                        {t(
                          'workflow.split.states.' +
                            workflowSplitStateKey(task.state)
                        )}
                      </span>
                    </div>
                    <p className="break-words text-xs text-low">
                      {task.node_id
                        ? getNodeLabel(task.node_id)
                        : t('workflow.split.unassigned')}
                    </p>
                    {task.depends_on.length > 0 ? (
                      <p className="text-xs text-low">
                        {t('workflow.split.dependsOn', {
                          titles: task.depends_on
                            .map(
                              (id) =>
                                batch.subtasks.find(
                                  (item) => item.subtask_id === id
                                )?.title || id
                            )
                            .join(', '),
                        })}
                      </p>
                    ) : null}
                    {task.error_text ? (
                      <p className="break-words text-xs text-error">
                        {task.error_text}
                      </p>
                    ) : null}
                    <WorkflowFileReferences
                      runId={run.id}
                      onOpenFile={onOpenFile}
                      files={[
                        { path: task.instruction_path },
                        ...(task.handoff_path
                          ? [{ path: task.handoff_path }]
                          : []),
                        ...(handoff?.files ?? []),
                      ]}
                    />
                    {onOpenSubtask ? (
                      <Button
                        type="button"
                        variant="ghost"
                        size="xs"
                        data-testid={`workflow-open-subtask-${task.id}`}
                        onClick={() => onOpenSubtask(task)}
                      >
                        {t('workflow.split.openExecutionSession')}
                      </Button>
                    ) : sessionHref ? (
                      <a
                        href={sessionHref}
                        className="inline-flex items-center gap-half rounded text-xs text-brand focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
                      >
                        {t('workflow.split.openExecutionSession')}
                        <ExternalLink className="h-3 w-3" aria-hidden="true" />
                      </a>
                    ) : null}
                  </li>
                );
              })}
            </ul>
          </div>
        ))}
      </div>
      <div className="text-xs text-low">
        {t('workflow.split.summaryExit')}:{' '}
        <span className="text-normal">
          {getNodeLabel(batch.summary_node_id)}
        </span>
      </div>
      <WorkflowPartialSummaryAction
        key={batch.id}
        run={run}
        batch={batch}
        projectId={projectId}
      />
    </section>
  );
}

function WorkflowPartialSummaryAction({
  run,
  batch,
  projectId,
}: {
  run: WorkflowRunResponse;
  batch: WorkflowSplitBatchView;
  projectId: string;
}) {
  const { t } = useTranslation('common');
  const hostId = useHostId();
  const navigation = useAppNavigation();
  const payloadRef = useRef<WorkflowSubmission | null>(null);
  const lockRef = useRef(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [acceptedRunId, setAcceptedRunId] = useState<string | null>(null);
  const allowed = canRequestWorkflowPartialSummary(run, batch);

  if (!allowed && !acceptedRunId && !error) return null;

  const submit = async () => {
    if (lockRef.current || !allowed || !run.attempt_id || acceptedRunId) return;
    lockRef.current = true;
    setPending(true);
    setError(null);
    try {
      payloadRef.current ??= buildWorkflowPartialSummarySubmission(
        run,
        batch,
        crypto.randomUUID(),
        t('workflow.split.partialSummaryRequest', { path: batch.manifest_path })
      );
      if (!payloadRef.current) return;
      const result = await workflowManagementApi.submit(
        run.attempt_id,
        payloadRef.current,
        hostId
      );
      setAcceptedRunId(result.run_id);
    } catch (submissionError) {
      setError(
        submissionError instanceof Error
          ? submissionError.message
          : t('workflow.split.partialSummaryFailed')
      );
    } finally {
      lockRef.current = false;
      setPending(false);
    }
  };

  return (
    <div
      className="space-y-half border-t border-secondary pt-base"
      data-testid="workflow-partial-summary-action"
    >
      <p className="text-xs leading-relaxed text-low">
        {t('workflow.split.partialSummaryHint')}
      </p>
      {error ? (
        <p className="break-words text-xs text-error" role="alert">
          {error}
        </p>
      ) : null}
      {acceptedRunId ? (
        <>
          <p className="text-xs text-normal" role="status">
            {t('workflow.split.partialSummaryAccepted')}
          </p>
          <Button
            variant="outline"
            size="sm"
            onClick={() =>
              navigation.goToProjectWorkflowRun(projectId, acceptedRunId, {
                hostId,
              })
            }
          >
            {t('workflow.split.openPartialSummary')}
          </Button>
        </>
      ) : (
        <Button
          variant="outline"
          size="sm"
          disabled={!allowed || pending}
          loading={pending}
          onClick={() => void submit()}
        >
          {t('workflow.split.requestPartialSummary')}
        </Button>
      )}
    </div>
  );
}
