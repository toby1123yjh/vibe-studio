import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AgentProviderCapability,
  type ExecutorConfig,
  type WorkflowTemplateResponse,
} from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/KeyboardDialog';
import { useWorkflowTemplateMutations } from '@/shared/hooks/useWorkflowTemplates';
import { WorkflowAgentExecutorField } from './WorkflowAgentExecutorField';
import { WorkflowMainSessionButton } from './WorkflowMainSessionButton';

const MAIN_AGENT_CAPABILITIES = [
  AgentProviderCapability.INITIAL_RUN,
  AgentProviderCapability.FOLLOW_UP,
  AgentProviderCapability.MCP,
] as const;

interface WorkflowMainAgentDialogProps {
  projectId: string;
  template: WorkflowTemplateResponse;
  taskId?: string;
  mainSessionWorkflowId?: string;
  onPublished?: (
    template: WorkflowTemplateResponse,
    previousRevision: number
  ) => void;
  onClose: () => void;
}

export function WorkflowMainAgentDialog({
  projectId,
  template,
  taskId,
  mainSessionWorkflowId,
  onPublished,
  onClose,
}: WorkflowMainAgentDialogProps) {
  const { t } = useTranslation('common');
  const { updateTemplate, isUpdating } = useWorkflowTemplateMutations();
  const [baseline, setBaseline] = useState(template);
  const [agentConfig, setAgentConfig] = useState<ExecutorConfig | null>(
    template.main_agent_config ?? null
  );
  const [prompt, setPrompt] = useState(template.main_agent_prompt ?? '');
  const [error, setError] = useState<string | null>(null);
  const readOnly = template.source === 'system';
  const dirty =
    JSON.stringify(agentConfig) !==
      JSON.stringify(baseline.main_agent_config ?? null) ||
    prompt !== (baseline.main_agent_prompt ?? '');

  const handleSave = async (): Promise<boolean> => {
    if (!dirty) return !!agentConfig;
    if (!agentConfig || readOnly || isUpdating) return false;
    setError(null);
    try {
      const saved = await updateTemplate({
        workflowId: template.id,
        payload: {
          expected_revision: baseline.revision,
          name: null,
          description: null,
          graph_json: null,
          main_agent_config: agentConfig,
          main_agent_prompt: prompt,
        },
      });
      onPublished?.(saved, baseline.revision);
      setBaseline(saved);
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return false;
    }
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        className="sm:max-w-2xl"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => event.stopPropagation()}
      >
        <DialogHeader>
          <DialogTitle>{t('workflow.management.mainAgent')}</DialogTitle>
          <DialogDescription>
            {t('workflow.management.mainAgentHint')}
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-base py-base">
          <WorkflowAgentExecutorField
            value={agentConfig}
            readOnly={readOnly || isUpdating}
            requiredCapabilities={MAIN_AGENT_CAPABILITIES}
            onChange={setAgentConfig}
          />
          <label
            htmlFor="workflow-main-prompt"
            className="text-base text-normal"
          >
            {t('workflow.management.prompt')}
          </label>
          <textarea
            id="workflow-main-prompt"
            value={prompt}
            onChange={(event) => setPrompt(event.target.value)}
            disabled={readOnly || isUpdating}
            rows={6}
            className="w-full resize-y rounded-md border border-secondary bg-secondary p-base text-base text-normal outline-none focus-visible:ring-1 focus-visible:ring-brand"
            placeholder={t('workflow.management.promptPlaceholder')}
          />
          <p className="text-base text-low">
            {t('workflow.management.discussionOnly')}
          </p>
          {error ? (
            <p role="alert" className="text-error">
              {error}
            </p>
          ) : null}
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={onClose}>
            {t('buttons.cancel')}
          </Button>
          {!readOnly ? (
            <Button
              loading={isUpdating}
              disabled={!agentConfig || !dirty}
              onClick={() => void handleSave()}
            >
              {t('buttons.save')}
            </Button>
          ) : null}
          <WorkflowMainSessionButton
            projectId={projectId}
            workflowId={mainSessionWorkflowId ?? template.id}
            taskId={taskId}
            disabled={!agentConfig || isUpdating}
            beforePrepare={handleSave}
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
