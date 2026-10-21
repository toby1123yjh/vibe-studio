import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { useTranslation } from 'react-i18next';
import type {
  ExecutorConfig,
  WorkflowNodeExecutionResponse,
} from 'shared/types';
import {
  AlertCircle,
  Clock,
  ExternalLink,
  Files,
  Play,
  Settings2,
} from 'lucide-react';
import { Button } from '@vibe/ui/components/Button';
import {
  WorkspaceFilePreviewActionsProvider,
  WorkspaceFilesInlineInspector,
  useWorkspaceFilePreviewState,
  type WorkspaceFilePreviewTarget,
} from '@/features/workspace-files';
import { cn } from '@/shared/lib/utils';
import {
  AgentRunSessionProvider,
  useAgentRunSession,
  type AgentRunSessionContextValue,
} from '@/features/agent-runtime/model/AgentRunSessionContext';
import { isCanonicalProjectionAvailable } from '@/features/agent-runtime/model/canonicalAgentTimeline';
import { ConversationBlockEntry } from '@/features/workspace-chat/ui/ConversationBlockEntry';
import type { WorkflowNodeData } from '../model/workflowGraph';
import { getWorkflowAgentDisplay } from '../model/workflowAgentDisplay';
import { coerceWorkflowNodeExecutorConfig } from '../model/workflowAgentNodeDraft';
import {
  formatWorkflowDuration,
  getNodeStatusTone,
  type StatusTone,
} from '../model/workflowRunView';
import type { CanonicalWorkflowNodeWorkView } from '../model/workflowRuntimeView';
import {
  getConditionRouterHumanPrompt,
  getConditionRouterReason,
  parseConditionRouterOutput,
} from '../model/workflowConditionRouterOutput';
import { workflowNodeStatusKey } from './workflowI18n';

interface WorkflowNodeSessionPanelProps {
  execution: WorkflowNodeExecutionResponse;
  workspaceId: string | null;
  sessionHref: string | null;
  workspaceHref: string | null;
  nodeTitle?: string;
  nodeData?: WorkflowNodeData | null;
  statusLabel?: string | null;
  hideHeader?: boolean;
  onEditConfig?: () => void;
  editConfigDisabled?: boolean;
  onRunStep?: () => void;
  runStepDisabled?: boolean;
  runStepTitle?: string;
  afterHeaderContent?: ReactNode;
  runtimeWork?: CanonicalWorkflowNodeWorkView | null;
  onInspectFiles?: () => void;
  inspectFilesDisabled?: boolean;
}

interface WorkflowNodeSessionHeaderProps {
  execution: WorkflowNodeExecutionResponse;
  sessionHref: string | null;
  workspaceHref: string | null;
  nodeTitle?: string;
  nodeData?: WorkflowNodeData | null;
  statusLabel?: string | null;
  onEditConfig?: () => void;
  editConfigDisabled?: boolean;
  onRunStep?: () => void;
  runStepDisabled?: boolean;
  runStepTitle?: string;
  runtimeWork?: CanonicalWorkflowNodeWorkView | null;
  onInspectFiles?: () => void;
  inspectFilesDisabled?: boolean;
}

const cockpitToneClassMap: Record<StatusTone, string> = {
  neutral: 'border-secondary bg-primary/60 text-low',
  active: 'border-brand/30 bg-brand/10 text-brand',
  success: 'border-success/35 bg-success/10 text-success',
  danger: 'border-error/45 bg-error/10 text-error',
  warning: 'border-warning/40 bg-warning/10 text-warning',
};

const cockpitDotClassMap: Record<StatusTone, string> = {
  neutral: 'bg-low',
  active: 'bg-brand',
  success: 'bg-success',
  danger: 'bg-error',
  warning: 'bg-warning',
};

export function WorkflowNodeSessionPanel(props: WorkflowNodeSessionPanelProps) {
  return (
    <AgentRunSessionProvider
      sessionId={props.execution.session_id ?? undefined}
    >
      <WorkflowNodeSessionPanelContent {...props} />
    </AgentRunSessionProvider>
  );
}

function WorkflowNodeSessionPanelContent({
  execution,
  workspaceId,
  sessionHref,
  workspaceHref,
  nodeTitle,
  nodeData,
  statusLabel,
  hideHeader = false,
  onEditConfig,
  editConfigDisabled,
  onRunStep,
  runStepDisabled,
  runStepTitle,
  afterHeaderContent,
  runtimeWork,
}: WorkflowNodeSessionPanelProps) {
  const nodeSessionId = execution.session_id;
  const agentRunId = runtimeWork?.active_agent_run_id ?? execution.agent_run_id;
  const projectionStatus =
    runtimeWork?.projection_status ?? execution.projection_status ?? null;
  const projectionAvailable = Boolean(
    agentRunId && projectionStatus === 'current'
  );
  const canonicalStream = useAgentRunSession();
  const [showFilesInspector, setShowFilesInspector] = useState(false);
  const { target, openTarget, clearTarget } = useWorkspaceFilePreviewState();
  const previewScopeKey = `${workspaceId ?? 'no-workspace'}:${nodeSessionId ?? 'no-session'}`;
  const preferredExecutorConfig = useMemo(
    () => coerceWorkflowNodeExecutorConfig(nodeData?.executor_config),
    [nodeData?.executor_config]
  );

  useEffect(() => {
    setShowFilesInspector(false);
    clearTarget();
  }, [clearTarget, previewScopeKey]);

  const handleInspectFiles = useCallback(() => {
    setShowFilesInspector(true);
  }, []);
  const handleOpenWorkspaceFilePreview = useCallback(
    (target: WorkspaceFilePreviewTarget) => {
      openTarget(target);
      setShowFilesInspector(true);
    },
    [openTarget]
  );

  if (!projectionAvailable || !agentRunId) {
    return (
      <WorkflowNodeSessionFallback
        execution={execution}
        sessionHref={sessionHref}
        workspaceHref={workspaceHref}
        nodeTitle={nodeTitle}
        nodeData={nodeData}
        statusLabel={statusLabel}
        hideHeader={hideHeader}
        onEditConfig={onEditConfig}
        editConfigDisabled={editConfigDisabled}
        onRunStep={onRunStep}
        runStepDisabled={runStepDisabled}
        runStepTitle={runStepTitle}
        afterHeaderContent={afterHeaderContent}
        runtimeWork={runtimeWork}
        canonicalStream={canonicalStream}
        agentRunId={agentRunId}
        projectionStatus={projectionStatus}
      />
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      {hideHeader ? null : (
        <WorkflowNodeSessionHeader
          execution={execution}
          sessionHref={sessionHref}
          workspaceHref={workspaceHref}
          nodeTitle={nodeTitle}
          nodeData={nodeData}
          statusLabel={statusLabel}
          onEditConfig={onEditConfig}
          editConfigDisabled={editConfigDisabled}
          onRunStep={onRunStep}
          runStepDisabled={runStepDisabled}
          runStepTitle={runStepTitle}
          runtimeWork={runtimeWork}
          onInspectFiles={handleInspectFiles}
          inspectFilesDisabled={!workspaceId}
        />
      )}
      {afterHeaderContent}
      <div className="min-h-0 flex-1 overflow-hidden">
        <WorkspaceFilePreviewActionsProvider
          enabled={Boolean(workspaceId)}
          onOpenWorkspaceFilePreview={handleOpenWorkspaceFilePreview}
        >
          {showFilesInspector ? (
            <WorkspaceFilesInlineInspector
              key={`${workspaceId}-${nodeSessionId}`}
              workspaceId={workspaceId!}
              target={target}
              source="workflow"
              sessionId={nodeSessionId}
              compact
              title="Files"
              onSelectFile={openTarget}
              onClearTarget={clearTarget}
              onClose={() => setShowFilesInspector(false)}
            />
          ) : (
            <WorkflowNodeCanonicalSession
              execution={execution}
              agentRunId={agentRunId}
              canonicalStream={canonicalStream}
              preferredExecutorConfig={preferredExecutorConfig}
              workspaceId={workspaceId}
            />
          )}
        </WorkspaceFilePreviewActionsProvider>
      </div>
    </div>
  );
}

function WorkflowNodeSessionHeader({
  execution,
  sessionHref,
  workspaceHref,
  nodeTitle,
  nodeData,
  statusLabel,
  onEditConfig,
  editConfigDisabled,
  onRunStep,
  runStepDisabled,
  runStepTitle,
  runtimeWork,
  onInspectFiles,
  inspectFilesDisabled,
}: WorkflowNodeSessionHeaderProps) {
  const { t } = useTranslation('common');
  const agentDisplay = getWorkflowAgentDisplay(nodeData ?? {});
  const title =
    nodeTitle?.trim() ||
    (execution.node_type === 'agent'
      ? t('workflow.nodeSession.agentStepSession')
      : t('workflow.nodeSession.nodeSession'));
  const sessionLabel = execution.session_id
    ? t('workflow.canvas.sessionReady')
    : t('workflow.nodeSession.sessionNotStarted');
  const statusText =
    statusLabel ||
    t(`workflow.nodeStatus.${workflowNodeStatusKey(execution.status)}`);
  const statusTone = getNodeStatusTone(execution.status);
  const canRunStep = Boolean(onRunStep) && !runStepDisabled;
  const canEditConfig = Boolean(onEditConfig) && !editConfigDisabled;
  const canInspectFiles = Boolean(onInspectFiles) && !inspectFilesDisabled;
  const openHref = sessionHref || workspaceHref;

  return (
    <div className="shrink-0 border-b border-secondary bg-panel/95 px-base py-base">
      <div className="flex items-start justify-between gap-base">
        <div className="min-w-0 space-y-2">
          <div className="min-w-0">
            <p className="text-[10px] font-semibold uppercase tracking-normal text-low">
              {t('workflow.nodeSession.cockpit')}
            </p>
            <h2 className="mt-1 truncate text-sm font-semibold text-high">
              {title}
            </h2>
          </div>

          <div className="flex flex-wrap items-center gap-1.5">
            <CockpitChip tone="active">{agentDisplay.agentLabel}</CockpitChip>
            <CockpitChip>{agentDisplay.modelLabel}</CockpitChip>
            {agentDisplay.reasoningLabel ? (
              <CockpitChip>{agentDisplay.reasoningLabel}</CockpitChip>
            ) : null}
            <CockpitChip tone={statusTone}>
              <span
                className={cn(
                  'mr-1.5 inline-block h-1.5 w-1.5 rounded-full',
                  cockpitDotClassMap[statusTone]
                )}
              />
              {statusText}
            </CockpitChip>
            <CockpitChip>{sessionLabel}</CockpitChip>
          </div>
        </div>

        <div className="flex shrink-0 flex-wrap justify-end gap-half">
          <Button
            type="button"
            size="xs"
            variant="outline"
            disabled={!canRunStep}
            title={
              canRunStep
                ? runStepTitle
                : (runStepTitle ?? t('workflow.canvas.runStepUnavailable'))
            }
            onClick={onRunStep}
            className="gap-1.5"
          >
            <Play className="h-3.5 w-3.5" />
            {t('workflow.nodeSession.runThisStep')}
          </Button>
          <Button
            type="button"
            size="xs"
            variant="outline"
            disabled={!canEditConfig}
            onClick={onEditConfig}
            className="gap-1.5"
          >
            <Settings2 className="h-3.5 w-3.5" />
            {t('workflow.nodeSession.editConfig')}
          </Button>
          <Button
            type="button"
            size="xs"
            variant="outline"
            disabled={!canInspectFiles}
            onClick={onInspectFiles}
            className="gap-1.5"
          >
            <Files className="h-3.5 w-3.5" />
            {t('workflow.nodeSession.inspectFiles')}
          </Button>
          {openHref ? (
            <Button asChild size="xs" variant="outline" className="gap-1.5">
              <a href={openHref}>
                <ExternalLink className="h-3.5 w-3.5" />
                {t('workflow.nodeSession.openWorkspace')}
              </a>
            </Button>
          ) : (
            <Button
              type="button"
              size="xs"
              variant="outline"
              disabled
              className="gap-1.5"
            >
              <ExternalLink className="h-3.5 w-3.5" />
              {t('workflow.nodeSession.openWorkspace')}
            </Button>
          )}
        </div>
      </div>

      <WorkflowNodeRunSummary execution={execution} runtimeWork={runtimeWork} />
      <WorkflowNodeTechnicalDetails execution={execution} />
    </div>
  );
}

function CockpitChip({
  children,
  tone = 'neutral',
}: {
  children: ReactNode;
  tone?: StatusTone;
}) {
  return (
    <span
      className={cn(
        'inline-flex max-w-[220px] items-center truncate rounded border px-1.5 py-0.5 text-[10px] font-medium leading-none',
        cockpitToneClassMap[tone]
      )}
    >
      {children}
    </span>
  );
}

function formatWorkflowTimestamp(
  value: string | null,
  locale: string
): string | null {
  if (!value) return null;
  const time = new Date(value);
  if (Number.isNaN(time.getTime())) return value;
  return time.toLocaleString(locale);
}

function formatElapsedMs(value: number): string {
  const diffSecs = Math.floor(Math.max(0, value) / 1000);
  const minutes = Math.floor(diffSecs / 60);
  const seconds = diffSecs % 60;

  if (minutes > 0) {
    return `${minutes}m ${seconds}s`;
  }
  return `${seconds}s`;
}

function WorkflowNodeRunSummary({
  execution,
  runtimeWork,
}: {
  execution: WorkflowNodeExecutionResponse;
  runtimeWork?: CanonicalWorkflowNodeWorkView | null;
}) {
  const { t } = useTranslation('common');
  const statusTone = runtimeWork?.active_slow
    ? 'warning'
    : getNodeStatusTone(execution.status);
  const { i18n } = useTranslation();
  const startedLabel = formatWorkflowTimestamp(
    execution.started_at,
    i18n.resolvedLanguage || 'en'
  );
  const finishedLabel = formatWorkflowTimestamp(
    execution.finished_at,
    i18n.resolvedLanguage || 'en'
  );
  const durationLabel = formatWorkflowDuration(
    execution.started_at,
    execution.finished_at
  );
  const runtimeDurationLabel =
    runtimeWork?.active_elapsed_ms != null
      ? formatElapsedMs(runtimeWork.active_elapsed_ms)
      : durationLabel;
  const hasError = execution.status === 'failed' || !!execution.error_text;
  const routerOutput = parseConditionRouterOutput(execution.output_text);
  const routerReadableOutput =
    getConditionRouterHumanPrompt(routerOutput) ??
    getConditionRouterReason(routerOutput);

  const summary = (() => {
    if (hasError) {
      return {
        title: t('workflow.nodeSession.lastRunFailed'),
        detail:
          execution.error_text ||
          routerReadableOutput ||
          execution.output_text ||
          t('workflow.nodeSession.failedWithoutMessage'),
      };
    }
    if (execution.status === 'running') {
      return {
        title: t('workflow.nodeSession.runningNow'),
        detail: startedLabel
          ? t('workflow.nodeSession.startedDuration', {
              started: startedLabel,
              duration: runtimeDurationLabel,
            })
          : t('workflow.nodeSession.agentWorking'),
      };
    }
    if (
      execution.status === 'awaiting_human' ||
      execution.status === 'awaiting_arena'
    ) {
      return {
        title: t(
          `workflow.nodeStatus.${workflowNodeStatusKey(execution.status)}`
        ),
        detail:
          routerReadableOutput ||
          execution.output_text ||
          t('workflow.nodeSession.waitingForDecision'),
      };
    }
    if (execution.status === 'succeeded') {
      return {
        title: t('workflow.nodeSession.lastRunSucceeded'),
        detail: finishedLabel
          ? t('workflow.nodeSession.finishedDuration', {
              finished: finishedLabel,
              duration: durationLabel,
            })
          : durationLabel,
      };
    }
    return {
      title: t('workflow.nodeSession.notStartedYet'),
      detail: execution.session_id
        ? t('workflow.nodeSession.readyForConversation')
        : t('workflow.nodeSession.noSessionYet'),
    };
  })();

  return (
    <div
      className={cn(
        'mt-base rounded border p-half text-xs',
        cockpitToneClassMap[hasError ? 'danger' : statusTone]
      )}
      role={hasError ? 'alert' : undefined}
    >
      <div className="flex items-center gap-half font-semibold text-high">
        {hasError ? (
          <AlertCircle className="h-3.5 w-3.5 text-error" />
        ) : (
          <Clock className="h-3.5 w-3.5 text-low" />
        )}
        <span>{summary.title}</span>
      </div>
      <p className="mt-1 line-clamp-3 whitespace-pre-wrap text-low">
        {summary.detail}
      </p>
    </div>
  );
}

function WorkflowNodeTechnicalDetails({
  execution,
}: {
  execution: WorkflowNodeExecutionResponse;
}) {
  const { t } = useTranslation('common');
  return (
    <details className="mt-half rounded border border-secondary bg-primary/50 px-half py-1 text-xs text-low">
      <summary className="cursor-pointer select-none text-[10px] font-semibold uppercase tracking-normal text-low">
        {t('workflow.nodeSession.technicalDetails')}
      </summary>
      <div className="mt-half grid gap-half">
        <TechnicalDetailRow label={t('workflow.runCanvas.nodeId')}>
          {execution.node_id}
        </TechnicalDetailRow>
        <TechnicalDetailRow label={t('workflow.runCanvas.executionId')}>
          {execution.id}
        </TechnicalDetailRow>
        <TechnicalDetailRow label={t('workflow.nodeSession.sessionId')}>
          {execution.session_id ?? t('workflow.dashboard.notStarted')}
        </TechnicalDetailRow>
        <TechnicalDetailRow
          label={t('workflow.nodeSession.orchestrationNodeExecutionId')}
        >
          {execution.orchestration_node_execution_id ??
            t('workflow.dashboard.notAvailable')}
        </TechnicalDetailRow>
        <TechnicalDetailRow label={t('workflow.nodeSession.agentRunId')}>
          {execution.agent_run_id ?? t('workflow.dashboard.notAvailable')}
        </TechnicalDetailRow>
        <TechnicalDetailRow label={t('workflow.nodeSession.projection')}>
          {execution.projection_status ?? t('workflow.dashboard.notAvailable')}
        </TechnicalDetailRow>
      </div>
    </details>
  );
}

function TechnicalDetailRow({
  children,
  label,
}: {
  children: ReactNode;
  label: string;
}) {
  return (
    <div className="grid grid-cols-[84px_minmax(0,1fr)] gap-half">
      <span className="text-low">{label}</span>
      <span className="break-all text-high">{children}</span>
    </div>
  );
}

function WorkflowNodeCanonicalSession({
  execution,
  agentRunId,
  canonicalStream,
  preferredExecutorConfig,
  workspaceId,
}: {
  execution: WorkflowNodeExecutionResponse;
  agentRunId: string;
  canonicalStream: AgentRunSessionContextValue;
  preferredExecutorConfig: ExecutorConfig | null;
  workspaceId: string | null;
}) {
  const { t } = useTranslation('common');
  const run = canonicalStream.timeline?.runs.find(
    (item) => item.summary.agent_run_id === agentRunId
  );
  const projectionAvailable = isCanonicalProjectionAvailable(
    run?.summary.state ?? null
  );
  const items = canonicalStream.conversation.entries.filter(
    (item) => item.canonical?.agentRunId === agentRunId
  );

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-primary p-base">
      {!projectionAvailable ? (
        <div className="mb-base rounded border border-warning/40 bg-warning/10 p-half text-xs text-warning">
          {t('workflow.nodeSession.projectionUnavailable', {
            defaultValue:
              'Canonical AgentRun projection is unavailable; actions are disabled.',
          })}
        </div>
      ) : null}
      <div className="mb-base grid gap-half text-xs text-low sm:grid-cols-2">
        <TechnicalDetailRow label={t('workflow.nodeSession.agentRunId')}>
          {agentRunId}
        </TechnicalDetailRow>
        <TechnicalDetailRow label={t('workflow.nodeSession.sessionId')}>
          {execution.session_id ?? t('workflow.dashboard.notAvailable')}
        </TechnicalDetailRow>
        <TechnicalDetailRow label={t('workflow.nodeSession.projection')}>
          {run?.summary.state.projection_status ??
            t('workflow.dashboard.notAvailable')}
        </TechnicalDetailRow>
        <TechnicalDetailRow label={t('workflow.nodeSession.runtimeConnection')}>
          {canonicalStream.isConnected
            ? t('workflow.nodeSession.connected', { defaultValue: 'Connected' })
            : t('workflow.nodeSession.disconnected', {
                defaultValue: 'Disconnected',
              })}
        </TechnicalDetailRow>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto space-y-half">
        {canonicalStream.error ? (
          <div className="rounded border border-error/50 bg-error/10 p-half text-xs text-error">
            {canonicalStream.error}
          </div>
        ) : null}
        {items.length === 0 ? (
          <div className="rounded border border-secondary bg-panel/50 p-base text-sm text-low">
            {t('workflow.nodeSession.noAgentResponse')}
          </div>
        ) : (
          items.map((item) => (
            <ConversationBlockEntry
              key={item.patchKey}
              blockId={item.conversationBlockId!}
              workspaceId={workspaceId ?? undefined}
            />
          ))
        )}
      </div>
      <div className="mt-base rounded border border-secondary bg-panel/60 p-half text-[10px] text-low">
        {preferredExecutorConfig
          ? t('workflow.nodeSession.nativeConfigLoaded', {
              defaultValue: 'Native executor configuration loaded.',
            })
          : t('workflow.nodeSession.nativeConfigUnavailable', {
              defaultValue: 'Native executor configuration is unavailable.',
            })}
      </div>
    </div>
  );
}

type WorkflowNodeSessionFallbackProps = Omit<
  WorkflowNodeSessionPanelProps,
  'workspaceId'
> & {
  canonicalStream?: AgentRunSessionContextValue;
  agentRunId?: string | null;
  projectionStatus?: string | null;
};

function WorkflowNodeSessionFallback({
  execution,
  sessionHref,
  workspaceHref,
  nodeTitle,
  nodeData,
  statusLabel,
  hideHeader,
  onEditConfig,
  editConfigDisabled,
  onRunStep,
  runStepDisabled,
  runStepTitle,
  afterHeaderContent,
  runtimeWork,
  canonicalStream,
  agentRunId,
  projectionStatus,
}: WorkflowNodeSessionFallbackProps) {
  const { t } = useTranslation('common');
  return (
    <div
      data-testid="workflow-node-session-panel"
      className="flex min-h-full flex-col"
    >
      {hideHeader ? null : (
        <WorkflowNodeSessionHeader
          execution={execution}
          sessionHref={sessionHref}
          workspaceHref={workspaceHref}
          nodeTitle={nodeTitle}
          nodeData={nodeData}
          statusLabel={statusLabel}
          onEditConfig={onEditConfig}
          editConfigDisabled={editConfigDisabled}
          onRunStep={onRunStep}
          runStepDisabled={runStepDisabled}
          runStepTitle={runStepTitle}
          runtimeWork={runtimeWork}
        />
      )}
      {afterHeaderContent}

      <div className="flex flex-1 flex-col gap-base overflow-y-auto p-base">
        {agentRunId ? (
          <div className="rounded border border-warning/40 bg-warning/10 p-half text-xs text-warning">
            {t('workflow.nodeSession.projectionUnavailable', {
              defaultValue:
                'Canonical AgentRun projection is unavailable; actions are disabled.',
            })}
          </div>
        ) : null}
        <WorkflowNodeOutputFallback
          execution={execution}
          agentRunId={agentRunId}
          projectionStatus={projectionStatus}
          streamError={canonicalStream?.error ?? null}
        />

        <div className="rounded border border-secondary bg-primary p-half">
          <h3 className="text-xs font-semibold uppercase text-low">
            {t('workflow.nodeSession.nodePrompt')}
          </h3>
          <pre className="mt-half whitespace-pre-wrap text-xs text-high">
            {execution.input_text || t('workflow.nodeSession.noPrompt')}
          </pre>
        </div>

        {execution.error_text ? (
          <div className="rounded border border-error/50 bg-error/10 p-half">
            <h3 className="text-xs font-semibold uppercase text-error">
              {t('workflow.dashboard.error')}
            </h3>
            <pre className="mt-half whitespace-pre-wrap text-xs text-error">
              {execution.error_text}
            </pre>
          </div>
        ) : null}
      </div>
    </div>
  );
}

function WorkflowNodeOutputFallback({
  execution,
  agentRunId,
  projectionStatus,
  streamError,
}: {
  execution: WorkflowNodeExecutionResponse;
  agentRunId?: string | null;
  projectionStatus?: string | null;
  streamError?: string | null;
}) {
  const { t } = useTranslation('common');
  const routerOutput = parseConditionRouterOutput(execution.output_text);
  const routerPrompt = getConditionRouterHumanPrompt(routerOutput);
  const routerReason = getConditionRouterReason(routerOutput);

  return (
    <div className="rounded border border-secondary bg-primary p-half">
      <h3 className="text-xs font-semibold uppercase text-low">
        {t('workflow.nodeSession.conversation')}
      </h3>
      <p data-testid="workflow-node-session-id" className="sr-only">
        {t('workflow.nodeSession.session', {
          id: execution.session_id ?? t('workflow.dashboard.notStarted'),
        })}
      </p>
      {agentRunId ? (
        <div className="mt-half space-y-1 text-[10px] text-low">
          <div>
            {t('workflow.nodeSession.agentRunId')}: {agentRunId}
          </div>
          <div>
            {t('workflow.nodeSession.projection')}:{' '}
            {projectionStatus ?? t('workflow.dashboard.notAvailable')}
          </div>
          {streamError ? <div className="text-error">{streamError}</div> : null}
        </div>
      ) : null}
      {routerOutput ? (
        <div className="mt-half space-y-half text-xs">
          {routerPrompt ? (
            <div className="rounded border border-warning/35 bg-warning/10 p-half text-high">
              {routerPrompt}
            </div>
          ) : null}
          {routerReason ? (
            <p className="whitespace-pre-wrap text-low">{routerReason}</p>
          ) : null}
          {routerOutput.raw_output ? (
            <details className="rounded border border-secondary bg-panel/50 p-half text-low">
              <summary className="cursor-pointer text-[10px] font-semibold uppercase tracking-normal">
                {t('workflow.nodeSession.rawRouterOutput')}
              </summary>
              <pre className="mt-half max-h-36 overflow-y-auto whitespace-pre-wrap text-[10px]">
                {routerOutput.raw_output}
              </pre>
            </details>
          ) : null}
        </div>
      ) : (
        <pre className="mt-half whitespace-pre-wrap text-xs text-high">
          {execution.output_text || t('workflow.nodeSession.noAgentResponse')}
        </pre>
      )}
    </div>
  );
}
