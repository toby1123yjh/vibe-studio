import {
  useCallback,
  useState,
  type MutableRefObject,
  type ReactNode,
} from 'react';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import {
  ArrowLeft,
  ChevronDown,
  ChevronRight,
  ExternalLink,
  FileText,
} from 'lucide-react';
import type { WorkflowContextFileView } from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import {
  FloatingPanel,
  FloatingPanelDescription,
  FloatingPanelHeader,
  FloatingPanelTitle,
} from '@vibe/ui/components/FloatingPanel';
import { WorkspaceSessionConversation } from '../../workspace-chat/ui/WorkspaceSessionConversation';
import type { WorkspaceConversationViewState } from '../../workspace-chat/model/conversationViewState';
import { createWorkflowApi } from '@/shared/lib/workflowApi';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { useArenaGroup } from '@/shared/hooks/useArenaGroup';
import {
  buildWorkflowConversationHref,
  getWorkflowOutputCount,
  resolveWorkflowConversationContext,
  resolveWorkflowSourceConversation,
  workflowConversationScopeKey,
  type WorkflowConversationTarget,
} from '../model/workflowConversationTarget';
import { workflowNodeStatusKey, workflowSplitStateKey } from './workflowI18n';
import {
  WorkflowFilePreview,
  type WorkflowFilePreviewTarget,
} from './WorkflowFileReferences';

export interface WorkflowConversationSavedView {
  expanded: boolean;
  preview: WorkflowFilePreviewTarget | null;
  conversation?: WorkspaceConversationViewState;
  previewScroll?: Record<string, number>;
}

function conversationStatusKey(target: WorkflowConversationTarget): string {
  if (target.kind === 'subtask') {
    return `workflow.split.states.${workflowSplitStateKey(target.status)}`;
  }
  if (target.kind === 'main') {
    return `workflow.runStatus.${workflowNodeStatusKey(target.status)}`;
  }
  return `workflow.nodeStatus.${workflowNodeStatusKey(target.status)}`;
}

interface WorkflowConversationPanelProps {
  target: WorkflowConversationTarget;
  choices: WorkflowConversationTarget[];
  projectId: string;
  taskId: string;
  agentLabel?: string;
  headerMetadata?: ReactNode;
  returnLabel?: string;
  onReturn?: () => void;
  onClose: () => void;
  onChoose: (target: WorkflowConversationTarget) => void;
  onOpenSource: (target: WorkflowConversationTarget) => void;
  savedViews: MutableRefObject<Map<string, WorkflowConversationSavedView>>;
  workList?: (
    openFile: (target: WorkflowFilePreviewTarget) => void
  ) => ReactNode;
}

/** One non-modal canvas overlay. It owns view state, never Session lifecycle. */
export function WorkflowConversationPanel(
  props: WorkflowConversationPanelProps
) {
  const hostId = useHostId();
  const key = workflowConversationScopeKey(hostId, props.target);
  return (
    <WorkflowConversationPanelForScope key={key} {...props} scopeKey={key} />
  );
}

function WorkflowConversationPanelForScope({
  target,
  choices,
  projectId,
  taskId,
  agentLabel,
  headerMetadata,
  returnLabel,
  onReturn,
  onClose,
  onChoose,
  onOpenSource,
  savedViews,
  scopeKey,
  workList,
}: WorkflowConversationPanelProps & { scopeKey: string }) {
  const { t } = useTranslation('common');
  const hostId = useHostId();
  const api = createWorkflowApi(hostId);
  const [view, setView] = useState<WorkflowConversationSavedView>(
    () => savedViews.current.get(scopeKey) ?? { expanded: false, preview: null }
  );
  const updateView = useCallback(
    (patch: Partial<WorkflowConversationSavedView>) => {
      // Persist before scheduling React state: list cleanup can report its
      // position during unmount, when a queued state update may be discarded.
      const next = {
        expanded: false,
        preview: null,
        ...savedViews.current.get(scopeKey),
        ...patch,
      };
      savedViews.current.set(scopeKey, next);
      setView(next);
    },
    [savedViews, scopeKey]
  );
  const sourceRun = useQuery({
    queryKey: ['workflow-conversation-source', hostId, target.runId],
    queryFn: () => api.getRun(target.runId),
    enabled: target.kind === 'source',
    refetchOnWindowFocus: false,
  });
  const candidateGroup = useArenaGroup(
    target.kind === 'candidate' ? target.arenaGroupId : null
  );
  const candidate = candidateGroup.data?.workspaces.find(
    (item) => item.candidate_id === target.candidateId
  );
  // An Arena container's accepted outputs are not each candidate's own files.
  const canReadContext =
    target.kind !== 'candidate' &&
    (target.kind === 'main' || Boolean(target.nodeExecutionId));
  const context = useQuery({
    queryKey: [
      'workflow-file-context',
      hostId,
      target.runId,
      target.key,
      target.nodeExecutionId,
      target.status,
    ],
    queryFn: ({ signal }) =>
      api.fileContext(target.runId, target.nodeExecutionId, signal),
    enabled: canReadContext,
    refetchOnWindowFocus: false,
    refetchInterval: [
      'starting',
      'running',
      'pending',
      'awaiting_human',
    ].includes(target.status)
      ? 3000
      : false,
  });
  const resolved = target.resolveSource
    ? resolveWorkflowConversationContext(target, context.data)
    : target.kind === 'source'
      ? resolveWorkflowSourceConversation(target, sourceRun.data)
      : target.kind === 'candidate' && candidateGroup.data
        ? {
            ...target,
            workspaceId: candidate?.workspace_id ?? null,
            sessionId: candidate?.session_id ?? null,
            status: candidate?.latest_agent_run_status ?? 'pending',
          }
        : target;
  const href = buildWorkflowConversationHref({
    hostId,
    projectId,
    taskId,
    workspaceId: resolved.workspaceId,
    sessionId: resolved.sessionId,
  });
  const sourceFailed = target.resolveSource
    ? context.isError
    : target.kind === 'source' && sourceRun.isError;
  const sourceLoading = target.resolveSource
    ? context.isPending
    : target.kind === 'source' && sourceRun.isPending;
  const outputs = getWorkflowOutputCount(context.data?.files ?? []);
  const openFile = (file: WorkflowFilePreviewTarget) =>
    updateView({ preview: file });
  const previewContent = view.preview ? (
    <div
      className="flex h-full min-h-0 flex-col bg-panel"
      data-testid="workflow-file-inline-preview"
    >
      <div className="flex min-w-0 shrink-0 items-center gap-half border-b border-secondary p-half">
        <Button
          variant="ghost"
          size="xs"
          onClick={() => updateView({ preview: null })}
        >
          <ArrowLeft className="mr-half h-3.5 w-3.5" aria-hidden="true" />
          {t('workflow.conversation.backToConversation')}
        </Button>
        <span
          className="min-w-0 flex-1 truncate font-mono text-xs text-normal"
          title={view.preview.path}
        >
          {view.preview.path}
        </span>
      </div>
      <div className="min-h-0 flex-1">
        <WorkflowFilePreview
          key={JSON.stringify(view.preview)}
          target={view.preview}
          initialScrollTop={
            savedViews.current.get(scopeKey)?.previewScroll?.[
              JSON.stringify(view.preview)
            ] ?? 0
          }
          onScrollTopChange={(top) => {
            const saved = savedViews.current.get(scopeKey) ?? view;
            savedViews.current.set(scopeKey, {
              ...saved,
              previewScroll: {
                ...saved.previewScroll,
                [JSON.stringify(view.preview)]: top,
              },
            });
          }}
        />
      </div>
    </div>
  ) : undefined;
  const deliverables = (
    <section
      className="shrink-0 border-b border-secondary"
      data-testid="workflow-deliverables"
    >
      <button
        type="button"
        className="flex w-full items-center gap-half px-base py-half text-left text-sm font-medium text-normal hover:bg-secondary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-brand"
        aria-expanded={view.expanded}
        aria-controls="workflow-deliverables-content"
        data-testid="workflow-deliverables-toggle"
        onClick={() => updateView({ expanded: !view.expanded })}
      >
        {view.expanded ? (
          <ChevronDown className="h-3.5 w-3.5" aria-hidden="true" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5" aria-hidden="true" />
        )}
        {t('workflow.conversation.deliverables', { count: outputs })}
        {context.isFetching && !context.data ? (
          <span className="ml-auto text-xs font-normal text-low">
            {t('workflow.split.loadingFile')}
          </span>
        ) : null}
      </button>
      {view.expanded ? (
        <div
          id="workflow-deliverables-content"
          className="max-h-[min(32vh,240px)] space-y-base overflow-y-auto border-t border-secondary px-base py-half text-xs"
        >
          {context.isError ? (
            <div className="space-y-half" role="alert">
              <p className="break-words text-error">
                {t('workflow.conversation.contextFailed')}
              </p>
              <Button
                variant="outline"
                size="xs"
                loading={context.isFetching}
                onClick={() => void context.refetch()}
              >
                {t('buttons.retry')}
              </Button>
            </div>
          ) : null}
          {(['output', 'input', 'control', 'handoff'] as const).map((role) => {
            const files =
              context.data?.files.filter((file) => file.role === role) ?? [];
            return files.length ? (
              <section key={role} className="space-y-half">
                <h3 className="font-medium text-low">
                  {t(`workflow.conversation.roles.${role}`)}
                </h3>
                <ul className="space-y-half">
                  {files.map((file) => (
                    <WorkflowContextFileRow
                      key={`${file.role}:${file.source_run_id}:${file.source_node_execution_id}:${file.path}`}
                      file={file}
                      onOpen={() =>
                        openFile({
                          kind: 'workflow',
                          runId: file.source_run_id,
                          path: file.path,
                        })
                      }
                      onOpenSource={onOpenSource}
                    />
                  ))}
                </ul>
              </section>
            ) : null;
          })}
          {canReadContext &&
          !context.isPending &&
          !context.isError &&
          outputs === 0 ? (
            <p className="text-low">{t('workflow.conversation.noOutputs')}</p>
          ) : null}
          {target.kind === 'candidate' ? (
            <p className="text-low">
              {t('workflow.conversation.candidateFilesHint')}
            </p>
          ) : null}
          {(context.data?.missing_subtasks ?? []).map((missing) => (
            <p
              key={`${missing.subtask_id}:${missing.title}`}
              className="break-words text-warning"
            >
              {missing.title} · {missing.reason}
            </p>
          ))}
          {!resolved.readOnly ? workList?.(openFile) : null}
        </div>
      ) : null}
    </section>
  );

  return (
    <FloatingPanel
      open
      portal={false}
      autoFocus={false}
      restoreFocus={false}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && view.preview && !event.defaultPrevented) {
          event.preventDefault();
          event.stopPropagation();
          updateView({ preview: null });
        }
      }}
      closeLabel={t('workflow.runCanvas.closeNodeDetails')}
      className="absolute bottom-6 right-6 top-6 [--vk-floating-panel-width:440px] !max-w-[calc(100%_-_3rem)] max-md:inset-0 max-md:[--vk-floating-panel-width:100%] max-md:!max-w-none max-md:rounded-none motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-right-2 motion-safe:duration-150"
      contentClassName="flex min-h-0 flex-col overflow-hidden"
      data-testid="workflow-conversation-panel"
      data-run-id={resolved.runId}
      data-session-id={resolved.sessionId ?? ''}
      data-workspace-id={resolved.workspaceId ?? ''}
      data-work-key={target.key}
    >
      <FloatingPanelHeader className="shrink-0">
        {onReturn ? (
          <button
            type="button"
            className="mb-half inline-flex items-center gap-half rounded text-xs text-brand focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
            onClick={onReturn}
          >
            <ArrowLeft className="h-3 w-3" aria-hidden="true" />
            {returnLabel ?? t('workflow.conversation.backToWork')}
          </button>
        ) : null}
        <div className="flex min-w-0 items-center gap-half">
          <FloatingPanelTitle
            className="min-w-0 flex-1 truncate"
            title={target.title}
          >
            {target.title}
          </FloatingPanelTitle>
          {href ? (
            <a
              href={href}
              className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded text-low hover:bg-secondary hover:text-high focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
              aria-label={t('workflow.runCanvas.openFullSession')}
              title={t('workflow.runCanvas.openFullSession')}
            >
              <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
            </a>
          ) : null}
        </div>
        <FloatingPanelDescription>
          {agentLabel && !resolved.readOnly ? `${agentLabel} · ` : ''}
          {t(conversationStatusKey(resolved))}
        </FloatingPanelDescription>
        {headerMetadata}
        {resolved.kind === 'source' ? (
          <p className="mt-half text-xs text-brand">
            {t('workflow.conversation.sourceReadOnly', {
              run: resolved.runId.slice(0, 8),
            })}
          </p>
        ) : null}
      </FloatingPanelHeader>
      {choices.length > 1 ? (
        <div className="shrink-0 border-b border-secondary px-base py-half">
          <select
            value={target.key}
            aria-label={t('workflow.conversation.chooseWork')}
            data-testid="workflow-conversation-work-selector"
            className="w-full min-w-0 rounded border border-secondary bg-secondary px-half py-half text-sm text-normal focus:outline-none focus:ring-2 focus:ring-brand"
            onChange={(event) => {
              const next = choices.find(
                (item) => item.key === event.currentTarget.value
              );
              if (next) onChoose(next);
            }}
          >
            {choices.map((choice) => (
              <option key={choice.key} value={choice.key}>
                {choice.title} · {t(conversationStatusKey(choice))}
              </option>
            ))}
          </select>
        </div>
      ) : null}
      <div className="relative flex min-h-0 flex-1 flex-col overflow-hidden">
        {resolved.workspaceId && resolved.sessionId ? (
          <WorkspaceSessionConversation
            workspaceId={resolved.workspaceId}
            sessionId={resolved.sessionId}
            readOnly={resolved.readOnly}
            beforeConversation={deliverables}
            previewContent={previewContent}
            viewState={view.conversation}
            onViewStateChange={(conversation) => updateView({ conversation })}
            onOpenWorkspaceFilePreview={(file) =>
              openFile({
                kind: 'workspace',
                workspaceId: file.workspaceId,
                repoId: file.repoId,
                path: file.path,
              })
            }
          />
        ) : (
          <div className="flex h-full min-h-0 flex-col">
            {deliverables}
            <div
              className="min-h-0 flex-1 space-y-base overflow-y-auto p-base text-sm text-low"
              data-testid="workflow-conversation-waiting"
              role="status"
            >
              <p>
                {sourceFailed
                  ? t('workflow.conversation.sourceFailed')
                  : sourceLoading
                    ? t('workflow.runPage.loadingRun')
                    : t('workflow.conversation.waitingSession')}
              </p>
              {sourceFailed ? (
                <Button
                  variant="outline"
                  size="xs"
                  onClick={() =>
                    void (target.resolveSource
                      ? context.refetch()
                      : sourceRun.refetch())
                  }
                >
                  {t('buttons.retry')}
                </Button>
              ) : null}
            </div>
            {previewContent ? (
              <div className="absolute inset-0 bg-panel">{previewContent}</div>
            ) : null}
          </div>
        )}
      </div>
    </FloatingPanel>
  );
}

function WorkflowContextFileRow({
  file,
  onOpen,
  onOpenSource,
}: {
  file: WorkflowContextFileView;
  onOpen: () => void;
  onOpenSource: (target: WorkflowConversationTarget) => void;
}) {
  const { t } = useTranslation('common');
  return (
    <li className="min-w-0">
      <button
        type="button"
        className="flex w-full items-start gap-half rounded p-half text-left text-brand hover:bg-secondary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
        onClick={onOpen}
      >
        <FileText className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
        <span className="min-w-0 break-all font-mono">{file.path}</span>
      </button>
      <details className="ml-5 text-low">
        <summary className="cursor-pointer rounded py-0.5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand">
          {t('workflow.runCanvas.reuseSource')}
        </summary>
        <div className="space-y-half break-all py-half">
          <p>
            {t('workflow.conversation.fileSource', {
              node: file.source_node_id ?? '—',
              run: file.source_run_id.slice(0, 8),
            })}
          </p>
          {file.source_session_id && file.source_node_execution_id ? (
            <button
              type="button"
              className="rounded text-brand focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
              onClick={() =>
                onOpenSource({
                  key: `source:${file.source_run_id}:${file.source_node_execution_id}`,
                  kind: 'source',
                  runId: file.source_run_id,
                  nodeId: file.source_node_id,
                  nodeExecutionId: file.source_node_execution_id,
                  workspaceId: file.source_workspace_id,
                  sessionId: file.source_session_id,
                  title: file.source_node_id ?? file.path,
                  status: 'succeeded',
                  readOnly: true,
                })
              }
            >
              {t('workflow.conversation.openSource')}
            </button>
          ) : null}
        </div>
      </details>
    </li>
  );
}
