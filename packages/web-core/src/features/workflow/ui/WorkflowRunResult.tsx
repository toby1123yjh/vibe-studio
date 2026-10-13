import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import type { WorkflowRunResponse } from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@vibe/ui/components/Dialog';
import { createWorkflowApi } from '@/shared/lib/workflowApi';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { WorkflowFileReferences } from './WorkflowFileReferences';
import { workflowSplitStateKey } from './workflowI18n';

export function WorkflowRunResult({
  run,
  projectId,
}: {
  run: WorkflowRunResponse;
  projectId: string;
}) {
  const { t } = useTranslation('settings');
  const { t: tc } = useTranslation('common');
  const hostId = useHostId();
  const navigation = useAppNavigation();
  const scopedWorkflowApi = createWorkflowApi(hostId);
  const [open, setOpen] = useState(false);
  const active = !['succeeded', 'failed', 'canceled'].includes(run.status);
  const files = useQuery({
    queryKey: ['workflow-file-changes', hostId, run.id, active],
    queryFn: () => scopedWorkflowApi.fileChanges(run.id),
    enabled: open,
    refetchInterval: open && active ? 4000 : false,
  });
  return (
    <>
      <Button variant="outline" size="sm" onClick={() => setOpen(true)}>
        {t('externalIntegrations.result')}
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent
          className="max-w-2xl"
          closeLabel={t('externalIntegrations.close')}
        >
          <DialogHeader>
            <DialogTitle>{t('externalIntegrations.result')}</DialogTitle>
            <DialogDescription>
              {t('externalIntegrations.resultHint')}
            </DialogDescription>
          </DialogHeader>
          <div className="max-h-[65vh] space-y-4 overflow-y-auto">
            {run.delivery ? (
              <section
                className="space-y-base rounded border border-secondary p-base"
                data-testid="workflow-delivery-completeness"
              >
                <h3
                  className={
                    run.delivery.completeness === 'partial'
                      ? 'font-medium text-warning'
                      : 'font-medium text-high'
                  }
                >
                  {tc(
                    `workflow.split.${run.delivery.completeness === 'partial' ? 'partialDelivery' : 'fullDelivery'}`
                  )}
                </h3>
                {run.delivery.completeness === 'partial' ? (
                  <p className="text-sm text-low">
                    {tc('workflow.split.partialDeliveryHint')}
                  </p>
                ) : null}
                {run.delivery.missing_subtasks.length > 0 ? (
                  <>
                    <h4 className="text-sm font-medium text-high">
                      {tc('workflow.split.missingSubtasks')}
                    </h4>
                    <ul
                      className="space-y-base"
                      data-testid="workflow-delivery-missing"
                    >
                      {run.delivery.missing_subtasks.map((task) => (
                        <li
                          key={task.subtask_id}
                          className="space-y-half text-sm"
                        >
                          <p className="flex items-start justify-between gap-base">
                            <span className="min-w-0 break-words text-high">
                              {task.title}
                            </span>
                            <span className="shrink-0 text-low">
                              {tc(
                                'workflow.split.states.' +
                                  workflowSplitStateKey(task.state)
                              )}
                            </span>
                          </p>
                          {task.reason ? (
                            <p className="break-words text-error">
                              {task.reason}
                            </p>
                          ) : null}
                        </li>
                      ))}
                    </ul>
                  </>
                ) : null}
                {run.delivery.source_run_id ? (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      if (run.delivery?.source_run_id) {
                        navigation.goToProjectWorkflowRun(
                          projectId,
                          run.delivery.source_run_id,
                          { hostId }
                        );
                      }
                    }}
                  >
                    {tc('workflow.split.openSourceRun')}
                  </Button>
                ) : null}
              </section>
            ) : null}
            {run.output_text && (
              <p className="whitespace-pre-wrap break-words text-normal">
                {run.output_text}
              </p>
            )}
            {run.error_text && (
              <p className="whitespace-pre-wrap break-words text-error">
                {run.error_text}
              </p>
            )}
            {run.request_path ? (
              <section className="space-y-half">
                <h3 className="font-medium text-high">
                  {tc('workflow.split.requestFile')}
                </h3>
                <WorkflowFileReferences
                  runId={run.id}
                  files={[{ path: run.request_path }]}
                />
              </section>
            ) : null}
            {run.handoffs.length > 0 ? (
              <section
                className="space-y-half"
                data-testid="workflow-delivery-files"
              >
                <h3 className="font-medium text-high">
                  {tc('workflow.split.handoffFiles')}
                </h3>
                <p className="text-sm text-low">
                  {tc('workflow.split.deliveryFilesHint')}
                </p>
                <WorkflowFileReferences
                  runId={run.id}
                  files={run.handoffs.flatMap((handoff) => [
                    { path: handoff.handoff_path },
                    ...handoff.files,
                  ])}
                />
              </section>
            ) : null}
            <section
              className="space-y-half"
              data-testid="workflow-agent-file-changes"
            >
              <h3 className="font-medium text-high">
                {t('externalIntegrations.agentFiles')}
              </h3>
              {files.isPending && (
                <p role="status">{t('externalIntegrations.loading')}</p>
              )}
              {files.isError && (
                <div role="alert" className="space-y-2 text-error">
                  <p>{t('externalIntegrations.filesFailed')}</p>
                  <Button
                    variant="outline"
                    loading={files.isFetching}
                    onClick={() => void files.refetch()}
                  >
                    {t('externalIntegrations.retry')}
                  </Button>
                </div>
              )}
              {files.data && (
                <>
                  <p className="text-base text-low" role="status">
                    {t(
                      `externalIntegrations.filesStatus.${files.data.collection_status}`
                    )}
                  </p>
                  <ul className="divide-y divide-border">
                    {files.data.files.map((file) => (
                      <li
                        key={`${file.change_type}:${file.path}`}
                        className="flex items-start gap-3 py-2 text-base"
                      >
                        <span className="shrink-0 rounded bg-secondary px-2 py-0.5 text-normal">
                          {t(
                            `externalIntegrations.fileKind.${file.change_type}`
                          )}
                        </span>
                        <span className="min-w-0 break-all font-mono text-high">
                          {file.path}
                        </span>
                      </li>
                    ))}
                  </ul>
                </>
              )}
            </section>
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
