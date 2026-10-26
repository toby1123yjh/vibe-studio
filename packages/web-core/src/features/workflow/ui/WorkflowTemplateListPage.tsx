import {
  useWorkflowTemplates,
  useWorkflowTemplateMutations,
} from '@/shared/hooks/useWorkflowTemplates';
import { scheduledTaskQueryKeys } from '@/shared/hooks/useScheduledTasks';
import { useWorkflowAttemptsForProjects } from '@/shared/hooks/useWorkflowAttempts';
import { useQueries } from '@tanstack/react-query';
import { useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  WorkflowAttemptResponse,
  WorkflowTemplateResponse,
} from 'shared/types';
import { useAppShellProjects } from '@/shared/hooks/useAppShellProjects';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { createScheduledTaskApi } from '@/shared/lib/scheduledTaskApi';
import { createDefaultWorkflowGraph } from '../model/workflowGraph';
import { getScheduledTaskSummary } from '../model/scheduledTaskPresentation';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { Button } from '@vibe/ui/components/Button';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@vibe/ui/components/Popover';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/KeyboardDialog';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@vibe/ui/components/Select';
import {
  DegradedState,
  EmptyState,
  ErrorState,
  LoadingState,
} from '@vibe/ui/components/StateSurface';
import {
  ArrowUpRight,
  Bot,
  CalendarClock,
  Folder,
  Loader2,
  MoreHorizontal,
  Plus,
  Workflow,
} from 'lucide-react';
import { getWorkflowDefaultGraphLabels } from './workflowI18n';
import { ScheduledTaskDialog } from './ScheduledTaskDialog';
import { WorkflowExternalAccessSwitch } from './WorkflowExternalAccessSwitch';
import { WorkflowMainAgentDialog } from './WorkflowMainAgentDialog';
import './workflow-directory.css';

export interface WorkflowTemplateListPageProps {
  projectId?: string;
}

type WorkflowListAction =
  | { kind: 'create' }
  | { kind: 'open' | 'mainAgent'; template: WorkflowTemplateResponse };

type WorkflowListEntry =
  | { kind: 'definition'; workflow: WorkflowTemplateResponse }
  | { kind: 'instance'; workflow: WorkflowAttemptResponse };

export function WorkflowTemplateListPage({
  projectId,
}: WorkflowTemplateListPageProps) {
  const { t, i18n } = useTranslation('common');
  const projectDialogId = useId();
  const hostId = useHostId();
  const projects = useAppShellProjects();
  const { data, isLoading, isFetching, error, refetch } = useWorkflowTemplates(
    projectId,
    { allProjects: !projectId }
  );
  const { createTemplate, isCreating } = useWorkflowTemplateMutations();
  const navigation = useAppNavigation();
  const [mainAgentTarget, setMainAgentTarget] = useState<{
    projectId: string;
    workflowId: string;
  } | null>(null);
  const [pendingAction, setPendingAction] = useState<{
    action: WorkflowListAction;
    projectId: string;
  } | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [openActionsFor, setOpenActionsFor] = useState<string | null>(null);
  const templates = (data?.workflows ?? []).filter(
    (template) => template.source === 'project'
  );
  const attemptQueries = useWorkflowAttemptsForProjects(
    projectId
      ? [projectId]
      : (projects?.items.map((project) => project.id) ?? [])
  );
  const entries: WorkflowListEntry[] = [
    ...attemptQueries.flatMap((query) =>
      (query.data?.attempts ?? []).map((workflow) => ({
        kind: 'instance' as const,
        workflow,
      }))
    ),
    ...templates.map((workflow) => ({
      kind: 'definition' as const,
      workflow,
    })),
  ];
  entries.sort((left, right) =>
    right.workflow.updated_at.localeCompare(left.workflow.updated_at)
  );
  const listError = error ?? attemptQueries.find((query) => query.error)?.error;
  const isListFetching =
    isFetching || attemptQueries.some((query) => query.isFetching);
  const projectDiscoveryFailed =
    !projectId && (projects?.isError || projects?.isFetchNextPageError);
  const isDiscoveringAttempts =
    (!projectId &&
      !!projects &&
      (projects.isLoading ||
        (projects.hasNextPage &&
          !projects.isError &&
          !projects.isFetchNextPageError))) ||
    attemptQueries.some((query) => query.isPending);
  const refetchList = () => {
    void refetch();
    attemptQueries.forEach((query) => void query.refetch());
  };
  const scheduleProjectIds = [
    ...new Set(
      templates.flatMap((template) => {
        const id = template.project_id ?? projectId;
        return id ? [id] : [];
      })
    ),
  ];
  const scheduleQueries = useQueries({
    queries: scheduleProjectIds.map((id) => ({
      queryKey: scheduledTaskQueryKeys.projectFiltered(
        id,
        { target_type: 'workflow' },
        hostId
      ),
      queryFn: () =>
        createScheduledTaskApi(hostId).list(id, { target_type: 'workflow' }),
    })),
  });
  const scheduledTasks = scheduleQueries.flatMap(
    (query) => query.data?.tasks ?? []
  );
  const projectNames = new Map(
    (projects?.items ?? []).map((project) => [project.id, project.name])
  );
  const needsMoreProjects =
    !projectId ||
    !!pendingAction ||
    templates.some(
      (template) =>
        template.project_id && !projectNames.has(template.project_id)
    );

  // Reuse shell discovery, including later pages, for names and create choices.
  // Project discovery must never gate loading the workflows themselves.
  useEffect(() => {
    if (
      needsMoreProjects &&
      projects?.hasNextPage &&
      !projects.isFetching &&
      !projects.isFetchingNextPage &&
      !projects.isError &&
      !projects.isFetchNextPageError
    ) {
      void projects.loadNextPage();
    }
  }, [needsMoreProjects, projects]);

  const performAction = async (
    action: WorkflowListAction,
    targetProjectId: string
  ) => {
    setActionError(null);
    try {
      if (action.kind === 'create') {
        const result = await createTemplate({
          projectId: targetProjectId,
          payload: {
            name: t('workflow.templates.newWorkflowName'),
            description: '',
            graph_json: JSON.stringify(
              createDefaultWorkflowGraph(getWorkflowDefaultGraphLabels(t))
            ),
          },
        });
        navigation.goToProjectWorkflowEdit(targetProjectId, result.id);
      } else if (action.kind === 'open') {
        navigation.goToProjectWorkflowEdit(targetProjectId, action.template.id);
      } else {
        setMainAgentTarget({
          projectId: targetProjectId,
          workflowId: action.template.id,
        });
      }
      setPendingAction(null);
    } catch (cause) {
      setActionError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const handleAction = (action: WorkflowListAction) => {
    const targetProjectId =
      (action.kind !== 'create' ? action.template.project_id : null) ??
      projectId;
    if (targetProjectId) {
      void performAction(action, targetProjectId);
    } else {
      setActionError(null);
      setPendingAction({ action, projectId: '' });
    }
  };

  if (!entries.length && (isLoading || isDiscoveringAttempts) && !listError) {
    return (
      <LoadingState
        className="vk-workflow-directory__state"
        title={t('workflow.templates.title')}
      />
    );
  }

  if (listError && !entries.length) {
    const message =
      listError instanceof Error ? listError.message : String(listError);
    return (
      <ErrorState
        className="vk-workflow-directory__state"
        title={t('workflow.templates.loadFailed', { message })}
        action={
          <Button
            type="button"
            variant="outline"
            loading={isListFetching}
            onClick={refetchList}
          >
            {t('buttons.retry')}
          </Button>
        }
      />
    );
  }

  const mainAgentTemplate = templates.find(
    (workflow) => workflow.id === mainAgentTarget?.workflowId
  );

  return (
    <div className="vk-workflow-directory">
      {mainAgentTemplate && mainAgentTarget ? (
        <WorkflowMainAgentDialog
          key={mainAgentTemplate.id}
          projectId={mainAgentTarget.projectId}
          template={mainAgentTemplate}
          onClose={() => setMainAgentTarget(null)}
        />
      ) : null}
      {pendingAction ? (
        <Dialog
          open
          role="dialog"
          aria-modal="true"
          aria-labelledby={`${projectDialogId}-title`}
          aria-describedby={`${projectDialogId}-description`}
          onOpenChange={(open) => {
            if (!open && !isCreating) setPendingAction(null);
          }}
        >
          <DialogContent>
            <DialogHeader>
              <DialogTitle id={`${projectDialogId}-title`}>
                {pendingAction.action.kind === 'create'
                  ? t('workflow.templates.newCanvas')
                  : pendingAction.action.template.name}
              </DialogTitle>
              <DialogDescription id={`${projectDialogId}-description`}>
                {t('workflow.management.chooseProjectHint')}
              </DialogDescription>
            </DialogHeader>
            <div className="flex flex-col gap-half py-base">
              <label
                htmlFor="workflow-project"
                className="text-base text-normal"
              >
                {t('workflow.management.project')}
              </label>
              <Select
                value={pendingAction.projectId}
                disabled={isCreating}
                onValueChange={(id) =>
                  setPendingAction({ ...pendingAction, projectId: id })
                }
              >
                <SelectTrigger id="workflow-project" className="w-full">
                  <SelectValue
                    placeholder={t('workflow.management.chooseProject')}
                  />
                </SelectTrigger>
                <SelectContent>
                  {(projects?.items ?? []).map((project) => (
                    <SelectItem key={project.id} value={project.id}>
                      {project.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {projects?.isLoading ? (
                <LoadingState
                  compact
                  title={t('workflow.management.loadingProjects')}
                />
              ) : null}
              {projects?.isError || projects?.isFetchNextPageError ? (
                <DegradedState
                  compact
                  title={t('workflow.management.projectsFailed')}
                  action={
                    <Button
                      variant="outline"
                      loading={projects.isFetching}
                      onClick={() => void projects.retry()}
                    >
                      {t('buttons.retry')}
                    </Button>
                  }
                />
              ) : null}
              {projects &&
              !projects.isLoading &&
              !projects.isError &&
              !projects.items.length ? (
                <p className="text-base text-low">
                  {t('workflow.management.noProjects')}
                </p>
              ) : null}
              {actionError ? (
                <p role="alert" className="text-error">
                  {actionError}
                </p>
              ) : null}
            </div>
            <DialogFooter>
              <Button
                variant="ghost"
                disabled={isCreating}
                onClick={() => setPendingAction(null)}
              >
                {t('buttons.cancel')}
              </Button>
              <Button
                loading={isCreating}
                disabled={
                  !pendingAction.projectId ||
                  !projects?.items.some(
                    (project) => project.id === pendingAction.projectId
                  )
                }
                onClick={() => {
                  if (!isCreating)
                    void performAction(
                      pendingAction.action,
                      pendingAction.projectId
                    );
                }}
              >
                {pendingAction.action.kind === 'create'
                  ? t('workflow.templates.createCanvas')
                  : t('buttons.continue')}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      ) : null}
      <header className="vk-workflow-directory__header">
        <h1>{t('workflow.templates.title')}</h1>
        <Button
          onClick={() => handleAction({ kind: 'create' })}
          disabled={isCreating}
          className="vk-workflow-directory__create"
        >
          <span className="inline-flex items-center gap-2">
            {isCreating ? (
              <Loader2
                aria-hidden="true"
                className="h-4 w-4 animate-spin motion-reduce:animate-none"
              />
            ) : (
              <Plus aria-hidden="true" className="h-4 w-4" />
            )}
            {t('workflow.templates.newCanvas')}
          </span>
        </Button>
      </header>

      {actionError && !pendingAction ? (
        <p role="alert" className="vk-workflow-directory__error">
          {actionError}
        </p>
      ) : null}
      {!pendingAction &&
      (projects?.isError || projects?.isFetchNextPageError) ? (
        <DegradedState
          compact
          className="vk-workflow-directory__notice"
          title={t('workflow.management.projectsFailed')}
          action={
            <Button
              variant="outline"
              size="sm"
              loading={projects.isFetching}
              onClick={() => void projects.retry()}
            >
              {t('buttons.retry')}
            </Button>
          }
        />
      ) : null}

      {listError ? (
        <DegradedState
          compact
          className="vk-workflow-directory__notice"
          title={t('workflow.templates.loadFailed', {
            message:
              listError instanceof Error
                ? listError.message
                : String(listError),
          })}
          action={
            <Button
              type="button"
              variant="outline"
              size="sm"
              loading={isListFetching}
              onClick={refetchList}
            >
              {t('buttons.retry')}
            </Button>
          }
        />
      ) : null}

      {entries.length === 0 && !projectDiscoveryFailed ? (
        <EmptyState
          className="vk-workflow-directory__empty"
          title={t('workflow.templates.emptyTitle')}
          description={t('workflow.templates.emptyDescription')}
          action={
            <Button
              type="button"
              loading={isCreating}
              onClick={() => handleAction({ kind: 'create' })}
            >
              {t('workflow.templates.createCanvas')}
            </Button>
          }
        />
      ) : (
        <div
          className="vk-workflow-directory__grid"
          role="list"
          aria-label={t('workflow.templates.title')}
        >
          {entries.map((entry) => {
            const { workflow } = entry;
            const template =
              entry.kind === 'definition' ? entry.workflow : null;
            const attempt = entry.kind === 'instance' ? entry.workflow : null;
            const targetProjectId = workflow.project_id ?? projectId;
            const scheduledTask =
              scheduledTasks.find(
                (task) =>
                  task.target_id === workflow.id &&
                  task.project_id === targetProjectId
              ) ?? null;
            const workflowName =
              workflow.name || t('workflow.templates.untitled');
            const entryKey = `${entry.kind}:${workflow.id}`;
            const updatedAt = new Date(workflow.updated_at);
            const projectName = workflow.project_id
              ? (projectNames.get(workflow.project_id) ?? workflow.project_id)
              : null;
            const openWorkflow = () => {
              if (attempt) {
                if (attempt.latest_run_id) {
                  navigation.goToProjectWorkflowRun(
                    attempt.project_id,
                    attempt.latest_run_id
                  );
                } else {
                  navigation.goToProjectWorkflowEdit(
                    attempt.project_id,
                    attempt.workflow_id
                  );
                }
              } else if (template) {
                handleAction({ kind: 'open', template });
              }
            };

            return (
              <div
                role="listitem"
                key={entryKey}
                className="vk-workflow-directory__card"
              >
                <div
                  role="button"
                  tabIndex={0}
                  className="vk-workflow-directory__open"
                  onClick={openWorkflow}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter' || event.key === ' ') {
                      event.preventDefault();
                      openWorkflow();
                    }
                  }}
                >
                  <span className="vk-workflow-directory__icon">
                    <Workflow size={18} aria-hidden="true" />
                  </span>
                  <div className="vk-workflow-directory__copy">
                    <h2 title={workflowName}>{workflowName}</h2>
                    {attempt || template?.description ? (
                      <p title={template?.description || undefined}>
                        {attempt
                          ? t('workflow.management.taskWorkflow')
                          : template?.description}
                      </p>
                    ) : null}
                  </div>
                  <div
                    className="vk-workflow-directory__project"
                    title={projectName ?? undefined}
                  >
                    <Folder size={14} aria-hidden="true" />
                    <span>
                      {projectName
                        ? t('workflow.management.projectLabel', {
                            name: projectName,
                          })
                        : t('workflow.management.allProjects')}
                    </span>
                  </div>
                  <div className="vk-workflow-directory__footer">
                    <span
                      className="vk-workflow-directory__status"
                      data-status={attempt?.status ?? 'definition'}
                    >
                      <span
                        className="vk-workflow-directory__status-dot"
                        aria-hidden="true"
                      />
                      <span>
                        {attempt
                          ? t(`workflow.management.status.${attempt.status}`)
                          : t('workflow.management.workflowDesign')}
                      </span>
                    </span>
                    <time
                      className="vk-workflow-directory__updated"
                      dateTime={workflow.updated_at}
                      title={updatedAt.toLocaleString(
                        i18n.resolvedLanguage || 'en'
                      )}
                    >
                      {updatedAt.toLocaleDateString(
                        i18n.resolvedLanguage || 'en',
                        { year: 'numeric', month: 'short', day: 'numeric' }
                      )}
                    </time>
                  </div>
                </div>
                <Popover
                  open={openActionsFor === entryKey}
                  onOpenChange={(open) =>
                    setOpenActionsFor(open ? entryKey : null)
                  }
                >
                  <PopoverTrigger asChild>
                    <Button
                      type="button"
                      variant="icon"
                      size="icon"
                      className="vk-workflow-directory__more"
                      aria-label={t('workflow.management.actionsFor', {
                        name: workflowName,
                      })}
                    >
                      <MoreHorizontal size={18} aria-hidden="true" />
                    </Button>
                  </PopoverTrigger>
                  <PopoverContent
                    align="end"
                    sideOffset={8}
                    className="vk-workflow-directory__actions"
                    aria-label={t('workflow.management.actionsFor', {
                      name: workflowName,
                    })}
                  >
                    <p
                      className="vk-workflow-directory__actions-title"
                      title={workflowName}
                    >
                      {workflowName}
                    </p>
                    {attempt ? (
                      <Button
                        type="button"
                        variant="ghost"
                        className="vk-workflow-directory__action"
                        onClick={() => {
                          setOpenActionsFor(null);
                          navigation.goToProjectTask(
                            attempt.project_id,
                            attempt.task_id
                          );
                        }}
                      >
                        <span className="vk-workflow-directory__action-label">
                          <ArrowUpRight size={16} aria-hidden="true" />
                          {t('workflow.management.openTask')}
                        </span>
                      </Button>
                    ) : template ? (
                      <>
                        <Button
                          type="button"
                          variant="ghost"
                          className="vk-workflow-directory__action"
                          disabled={!!error}
                          onClick={() => {
                            setOpenActionsFor(null);
                            handleAction({ kind: 'mainAgent', template });
                          }}
                        >
                          <span className="vk-workflow-directory__action-label">
                            <Bot size={16} aria-hidden="true" />
                            {t('workflow.management.mainAgent')}
                          </span>
                        </Button>
                        {targetProjectId ? (
                          <div className="vk-workflow-directory__schedule">
                            <Button
                              type="button"
                              variant="ghost"
                              className="vk-workflow-directory__action"
                              onClick={() => {
                                setOpenActionsFor(null);
                                void ScheduledTaskDialog.show({
                                  projectId: targetProjectId,
                                  workflowId: template.id,
                                  workflowName,
                                  existingTask: scheduledTask,
                                });
                              }}
                            >
                              <span className="vk-workflow-directory__action-label">
                                <CalendarClock size={16} aria-hidden="true" />
                                {t('workflow.schedule.button')}
                              </span>
                            </Button>
                            <p>
                              {t('workflow.schedule.summaryPrefix', {
                                summary: getScheduledTaskSummary(
                                  scheduledTask,
                                  t
                                ),
                              })}
                            </p>
                          </div>
                        ) : null}
                        <div className="vk-workflow-directory__external-access">
                          <WorkflowExternalAccessSwitch
                            workflowId={template.id}
                            enabled={template.external_enabled ?? false}
                            disabled={!!error}
                          />
                        </div>
                      </>
                    ) : null}
                  </PopoverContent>
                </Popover>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
