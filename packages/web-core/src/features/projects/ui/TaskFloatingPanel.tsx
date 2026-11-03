import { useEffect, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
  ArrowLeft,
  ArrowRight,
  Bot,
  ChevronDown,
  LoaderCircle,
  Sparkles,
  Workflow,
  X,
} from 'lucide-react';
import type { ExecutionSummary } from 'shared/types';
import type { Task, ProjectStatus, Tag } from 'shared/remote-types';
import {
  executionKindLabel,
  executionStatusLabel,
} from '../model/project-kanban';
import {
  ExecutionDeleteButton,
  type ExecutionDeletionActions,
} from './ExecutionDeleteButton';

interface TaskFloatingPanelProps extends ExecutionDeletionActions {
  task: Task;
  statuses: ProjectStatus[];
  tags: Tag[];
  selectedTagIds: Set<string>;
  executions: ExecutionSummary[];
  error: string | null;
  busyAction: 'agent' | 'workflow' | 'arena' | null;
  agentUnavailableReason: string | null;
  workflowUnavailableReason: string | null;
  arenaUnavailableReason: string | null;
  relationships: ReactNode;
  comments: ReactNode;
  onClose(): void;
  onOpenExecution(execution: ExecutionSummary): void;
  getExecutionUnavailableReason(execution: ExecutionSummary): string | null;
  onCreateAgent(): void;
  onCreateWorkflow(): void;
  onCreateArena(): void;
  onUpdateDescription(description: string | null): void;
  onUpdateStatus(statusId: string): void;
  onToggleTag(tagId: string): void;
}

function ExecutionRow({
  execution,
  onOpen,
  unavailableReason,
  onDeleteExecution,
  deletingSessionId,
}: ExecutionDeletionActions & {
  execution: ExecutionSummary;
  onOpen(): void;
  unavailableReason: string | null;
}) {
  return (
    <div className="vk-task-action-row">
      <button
        type="button"
        className="vk-task-task-row"
        aria-disabled={unavailableReason ? true : undefined}
        title={unavailableReason ?? undefined}
        onClick={() => {
          if (!unavailableReason) onOpen();
        }}
      >
        <span className="vk-task-task-row__copy">
          <strong>{execution.title}</strong>
          <small>{executionKindLabel(execution.execution_kind)}</small>
        </span>
        <span
          className="vk-task-task-row__status"
          data-status={execution.status}
        >
          {executionStatusLabel(execution.status)}
        </span>
        <ArrowRight aria-hidden="true" size={16} />
      </button>
      <ExecutionDeleteButton
        execution={execution}
        onDeleteExecution={onDeleteExecution}
        deletingSessionId={deletingSessionId}
      />
    </div>
  );
}

export function TaskFloatingPanel({
  task,
  statuses,
  tags,
  selectedTagIds,
  executions,
  error,
  busyAction,
  agentUnavailableReason,
  workflowUnavailableReason,
  arenaUnavailableReason,
  relationships,
  comments,
  onClose,
  onOpenExecution,
  onDeleteExecution,
  deletingSessionId,
  getExecutionUnavailableReason,
  onCreateAgent,
  onCreateWorkflow,
  onCreateArena,
  onUpdateDescription,
  onUpdateStatus,
  onToggleTag,
}: TaskFloatingPanelProps) {
  const { t } = useTranslation('common');
  const [informationOpen, setInformationOpen] = useState(false);
  const [description, setDescription] = useState(task.description ?? '');

  useEffect(() => {
    setInformationOpen(false);
    setDescription(task.description ?? '');
  }, [task.id, task.description]);

  return (
    <div className="vk-task-panel__layout">
      <header className="vk-task-panel__header">
        <button
          type="button"
          className="vk-task-panel__back"
          onClick={onClose}
          aria-label="Back to board"
        >
          <ArrowLeft aria-hidden="true" size={18} />
        </button>
        <span>{task.simple_id}</span>
        <button
          type="button"
          onClick={onClose}
          aria-label={t('kanban.closePanel')}
        >
          <X aria-hidden="true" size={18} />
        </button>
      </header>

      <div className="vk-task-panel__body">
        <h1>{task.title}</h1>

        <section
          className="vk-task-panel__tasks"
          aria-labelledby="task-executions"
        >
          <div className="vk-task-panel__section-heading">
            <h2 id="task-executions">
              {t('taskDetails.executions', 'Executions')}
            </h2>
            <span>{executions.length}</span>
          </div>
          {executions.length === 0 ? (
            <p className="vk-task-panel__empty">
              {t(
                'taskDetails.noExecutions',
                'No executions have been started for this task.'
              )}
            </p>
          ) : (
            <div className="vk-task-task-list">
              {executions.map((execution) => (
                <ExecutionRow
                  key={execution.id}
                  execution={execution}
                  onOpen={() => onOpenExecution(execution)}
                  onDeleteExecution={onDeleteExecution}
                  deletingSessionId={deletingSessionId}
                  unavailableReason={getExecutionUnavailableReason(execution)}
                />
              ))}
            </div>
          )}
        </section>

        <section
          className="vk-task-panel__execute"
          aria-labelledby="new-execution"
        >
          <h2 id="new-execution">
            {t('taskDetails.newExecution', 'New execution')}
          </h2>
          <div>
            <button
              type="button"
              onClick={() => {
                if (!agentUnavailableReason) onCreateAgent();
              }}
              disabled={busyAction === 'agent'}
              aria-busy={busyAction === 'agent'}
              aria-disabled={agentUnavailableReason ? true : undefined}
              title={agentUnavailableReason ?? undefined}
            >
              {busyAction === 'agent' ? (
                <LoaderCircle
                  aria-hidden="true"
                  size={17}
                  className="vk-spin"
                />
              ) : (
                <Bot aria-hidden="true" size={17} />
              )}
              <span>{t('taskDetails.singleAgent', 'Single agent')}</span>
            </button>
            <button
              type="button"
              onClick={() => {
                if (!workflowUnavailableReason) onCreateWorkflow();
              }}
              disabled={busyAction === 'workflow'}
              aria-busy={busyAction === 'workflow'}
              aria-disabled={workflowUnavailableReason ? true : undefined}
              title={workflowUnavailableReason ?? undefined}
            >
              {busyAction === 'workflow' ? (
                <LoaderCircle
                  aria-hidden="true"
                  size={17}
                  className="vk-spin"
                />
              ) : (
                <Workflow aria-hidden="true" size={17} />
              )}
              <span>{t('taskDetails.workflow', 'Workflow')}</span>
            </button>
            <button
              type="button"
              onClick={() => {
                if (!arenaUnavailableReason) onCreateArena();
              }}
              disabled={busyAction === 'arena'}
              aria-busy={busyAction === 'arena'}
              aria-disabled={arenaUnavailableReason ? true : undefined}
              title={arenaUnavailableReason ?? undefined}
            >
              {busyAction === 'arena' ? (
                <LoaderCircle
                  aria-hidden="true"
                  size={17}
                  className="vk-spin"
                />
              ) : (
                <Sparkles aria-hidden="true" size={17} />
              )}
              <span>{t('taskDetails.arena', 'Arena')}</span>
            </button>
          </div>
          {agentUnavailableReason ||
          workflowUnavailableReason ||
          arenaUnavailableReason ? (
            <small className="vk-task-panel__capability-note">
              {[
                agentUnavailableReason,
                workflowUnavailableReason,
                arenaUnavailableReason,
              ]
                .filter((reason): reason is string => Boolean(reason))
                .join(' ')}
            </small>
          ) : null}
        </section>

        {error ? (
          <p className="vk-task-panel__error" role="alert">
            {error}
          </p>
        ) : null}

        <section className="vk-task-information">
          <button
            type="button"
            className="vk-task-information__trigger"
            aria-expanded={informationOpen}
            aria-controls="task-information-body"
            onClick={() => setInformationOpen((open) => !open)}
          >
            <span>{t('taskDetails.information', 'Task information')}</span>
            <ChevronDown aria-hidden="true" size={17} />
          </button>
          {informationOpen ? (
            <div
              id="task-information-body"
              className="vk-task-information__body"
            >
              <label>
                <span>{t('kanban.status', 'Status')}</span>
                <select
                  value={task.status_id}
                  onChange={(event) => onUpdateStatus(event.target.value)}
                >
                  {statuses.map((status) => (
                    <option key={status.id} value={status.id}>
                      {status.name}
                    </option>
                  ))}
                </select>
              </label>

              <label>
                <span>{t('taskDetails.description', 'Description')}</span>
                <textarea
                  value={description}
                  rows={5}
                  placeholder={t('kanban.issueDescriptionPlaceholder')}
                  onChange={(event) => setDescription(event.target.value)}
                  onBlur={() => {
                    const next = description.trim() || null;
                    if (next !== task.description) onUpdateDescription(next);
                  }}
                />
              </label>

              <fieldset>
                <legend>{t('kanban.tags', 'Tags')}</legend>
                <div className="vk-task-information__tags">
                  {tags.length === 0 ? (
                    <span>{t('kanban.noTagsAvailable')}</span>
                  ) : (
                    tags.map((tag) => (
                      <button
                        key={tag.id}
                        type="button"
                        aria-pressed={selectedTagIds.has(tag.id)}
                        onClick={() => onToggleTag(tag.id)}
                      >
                        {tag.name}
                      </button>
                    ))
                  )}
                </div>
              </fieldset>

              <div className="vk-task-information__legacy-section">
                {relationships}
              </div>
              <div className="vk-task-information__legacy-section">
                {comments}
              </div>
            </div>
          ) : null}
        </section>
      </div>
    </div>
  );
}
