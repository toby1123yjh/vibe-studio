import { useTranslation } from 'react-i18next';
import { AlertCircle, CheckCircle, Clock, ExternalLink } from 'lucide-react';
import type { WorkflowCoordinationEventView } from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import {
  WorkflowFileReferences,
  type WorkflowFilePreviewTarget,
} from './WorkflowFileReferences';
import { getWorkflowCoordinationLabel } from '../model/workflowPresentation';

/** Additional coordination facts retain the failed source and its original chat. */
export function WorkflowCoordinationNotice({
  event,
  onOpenMainSession,
  onOpenFile,
}: {
  event: WorkflowCoordinationEventView;
  onOpenMainSession?: () => void;
  onOpenFile?: (file: WorkflowFilePreviewTarget) => void;
}) {
  const { t } = useTranslation('common');
  const Icon =
    event.state === 'completed'
      ? CheckCircle
      : event.state === 'failed'
        ? AlertCircle
        : Clock;
  return (
    <section
      className="space-y-half rounded border border-secondary bg-secondary/20 p-base"
      data-testid="workflow-coordination-notice"
    >
      <p
        className="flex items-center gap-half text-xs font-medium text-high"
        role="status"
      >
        <Icon className="h-3.5 w-3.5 shrink-0 text-low" aria-hidden="true" />
        {getWorkflowCoordinationLabel(event.state, t)}
      </p>
      {event.state === 'completed' ? (
        <p className="text-xs text-low">
          {t('workflow.coordination.outcomeHint')}
        </p>
      ) : null}
      {event.error_text ? (
        <p className="whitespace-pre-wrap break-words text-xs text-low">
          {event.error_text}
        </p>
      ) : null}
      {event.evidence_path ? (
        <WorkflowFileReferences
          runId={event.run_id}
          files={[{ path: event.evidence_path }]}
          onOpenFile={onOpenFile}
        />
      ) : null}
      {onOpenMainSession ? (
        <Button
          type="button"
          variant="outline"
          size="xs"
          onClick={onOpenMainSession}
        >
          <ExternalLink className="mr-half h-3.5 w-3.5" aria-hidden="true" />
          {t('workflow.coordination.openMainSession')}
        </Button>
      ) : null}
    </section>
  );
}
