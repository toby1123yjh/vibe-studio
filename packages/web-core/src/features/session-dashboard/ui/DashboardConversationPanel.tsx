import { useCallback, useRef, useState, type MutableRefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { ArrowLeft, ExternalLink } from 'lucide-react';
import {
  FloatingPanel,
  FloatingPanelDescription,
  FloatingPanelHeader,
  FloatingPanelTitle,
} from '@vibe/ui/components/FloatingPanel';
import { WorkspaceSessionConversation } from '@/features/workspace-chat/ui/WorkspaceSessionConversation';
import type { WorkspaceConversationViewState } from '@/features/workspace-chat/model/conversationViewState';
import { CONVERSATION_PANEL_SIZING } from '@/shared/lib/floatingPanelSizing';
import {
  WorkflowConversationPanel,
  type WorkflowConversationSavedView,
} from '@/features/workflow/ui/WorkflowConversationPanel';
import type { WorkflowConversationTarget } from '@/features/workflow/model/workflowConversationTarget';
import {
  WorkflowFilePreview,
  type WorkflowFilePreviewTarget,
} from '@/features/workflow/ui/WorkflowFileReferences';
import {
  dashboardConversationKey,
  dashboardProjectTitle,
  dashboardTargetHref,
} from '../model/dashboard';
import type { DashboardConversationSelection } from './DashboardTable';

interface SavedView {
  conversation?: WorkspaceConversationViewState;
  preview: WorkflowFilePreviewTarget | null;
  previewScroll?: Record<string, number>;
}

export function DashboardConversationPanel({
  selection,
  scopeKey,
  hostId,
  onClose,
  onNavigate,
}: {
  selection: DashboardConversationSelection | null;
  scopeKey: string;
  hostId: string | null;
  onClose: () => void;
  onNavigate: (href: string) => void;
}) {
  const savedViews = useRef(new Map<string, SavedView>());
  const workflowViews = useRef(
    new Map<string, WorkflowConversationSavedView>()
  );
  if (!selection) return null;
  return (
    <SelectedConversation
      key={dashboardConversationKey(scopeKey, selection.target)}
      selection={selection}
      scopeKey={scopeKey}
      hostId={hostId}
      savedViews={savedViews}
      workflowViews={workflowViews}
      onClose={onClose}
      onNavigate={onNavigate}
    />
  );
}

function RunReference({ runId }: { runId: string | null }) {
  const { t } = useTranslation('common');
  return runId ? (
    <p
      className="session-dashboard__run-reference"
      title={runId}
      data-testid="dashboard-selected-run"
    >
      {t('sessionDashboard.context.run')} <code>{runId}</code>
    </p>
  ) : null;
}

function SelectedConversation({
  selection,
  scopeKey,
  hostId,
  savedViews,
  workflowViews,
  onClose,
  onNavigate,
}: {
  selection: DashboardConversationSelection;
  scopeKey: string;
  hostId: string | null;
  savedViews: MutableRefObject<Map<string, SavedView>>;
  workflowViews: MutableRefObject<Map<string, WorkflowConversationSavedView>>;
  onClose: () => void;
  onNavigate: (href: string) => void;
}) {
  const [sourceTarget, setSourceTarget] =
    useState<WorkflowConversationTarget | null>(null);
  const key = dashboardConversationKey(scopeKey, selection.target);
  const target = selection.target;
  const workflowKind =
    selection.role === 'subtask'
      ? 'subtask'
      : selection.role === 'main_agent'
        ? 'main'
        : selection.role === 'source' || selection.readOnly
          ? 'source'
          : 'node';
  // Only real workflow bindings may enter workflow's delivery/source wrapper.
  const workflowTarget: WorkflowConversationTarget | null =
    target.workflow_run_id &&
    selection.role !== 'arena_candidate' &&
    (target.node_execution_id || workflowKind === 'main')
      ? {
          key,
          kind: workflowKind,
          runId: target.workflow_run_id,
          nodeId: target.workflow_node_id,
          nodeExecutionId: target.node_execution_id,
          workspaceId: target.workspace_id,
          sessionId: target.session_id,
          title: selection.title,
          status: selection.status,
          readOnly: selection.readOnly,
        }
      : null;
  if (workflowTarget) {
    const shownTarget = sourceTarget ?? workflowTarget;
    return (
      <WorkflowConversationPanel
        key={`${key}:${shownTarget.key}`}
        target={shownTarget}
        choices={[shownTarget]}
        projectId={selection.owner.project?.id ?? ''}
        taskId={selection.owner.task?.id ?? ''}
        headerMetadata={
          !sourceTarget ? (
            <RunReference runId={target.agent_run_id} />
          ) : undefined
        }
        onClose={onClose}
        onChoose={() => {}}
        onOpenSource={setSourceTarget}
        onReturn={sourceTarget ? () => setSourceTarget(null) : undefined}
        savedViews={workflowViews}
      />
    );
  }
  return (
    <BoundConversationPanel
      key={key}
      scopeKey={key}
      selection={selection}
      hostId={hostId}
      savedViews={savedViews}
      onClose={onClose}
      onNavigate={onNavigate}
    />
  );
}

function BoundConversationPanel({
  selection,
  scopeKey,
  hostId,
  savedViews,
  onClose,
  onNavigate,
}: {
  selection: DashboardConversationSelection;
  scopeKey: string;
  hostId: string | null;
  savedViews: MutableRefObject<Map<string, SavedView>>;
  onClose: () => void;
  onNavigate: (href: string) => void;
}) {
  const { t } = useTranslation('common');
  const [view, setView] = useState<SavedView>(
    () => savedViews.current.get(scopeKey) ?? { preview: null }
  );
  const updateView = useCallback(
    (patch: Partial<SavedView>) => {
      const next = {
        preview: null,
        ...savedViews.current.get(scopeKey),
        ...patch,
      };
      savedViews.current.set(scopeKey, next);
      setView(next);
    },
    [savedViews, scopeKey]
  );
  const target = selection.target;
  const href = dashboardTargetHref(hostId, target, selection.owner);
  const preview = view.preview ? (
    <div className="flex h-full min-h-0 flex-col bg-panel">
      <div className="shrink-0 border-b border-secondary p-half">
        <button
          type="button"
          className="session-dashboard__text-button"
          onClick={() => updateView({ preview: null })}
        >
          <ArrowLeft size={14} aria-hidden="true" />
          {t('workflow.conversation.backToConversation')}
        </button>
      </div>
      <WorkflowFilePreview
        target={view.preview}
        initialScrollTop={view.previewScroll?.[JSON.stringify(view.preview)]}
        onScrollTopChange={(scrollTop) =>
          updateView({
            previewScroll: {
              ...view.previewScroll,
              [JSON.stringify(view.preview)]: scrollTop,
            },
          })
        }
      />
    </div>
  ) : undefined;
  return (
    <FloatingPanel
      open
      autoFocus={false}
      restoreFocus={false}
      portal={false}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      closeLabel={t('sessionDashboard.closeConversation')}
      resize={{
        ...CONVERSATION_PANEL_SIZING,
        label: t('panels.resizeWidth', { defaultValue: 'Resize panel width' }),
      }}
      className="absolute bottom-6 right-6 top-6 !max-w-[calc(100%_-_3rem)] max-md:!max-w-none motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-right-2 motion-safe:duration-150"
      contentClassName="flex min-h-0 flex-col overflow-hidden"
      data-testid="dashboard-conversation-panel"
      data-workspace-id={target.workspace_id}
      data-session-id={target.session_id}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && view.preview && !event.defaultPrevented) {
          event.preventDefault();
          event.stopPropagation();
          updateView({ preview: null });
        }
      }}
    >
      <FloatingPanelHeader className="shrink-0">
        <div className="flex min-w-0 items-center gap-half">
          <FloatingPanelTitle
            className="min-w-0 flex-1 truncate"
            title={selection.title}
          >
            {selection.title}
          </FloatingPanelTitle>
          {href ? (
            <button
              type="button"
              className="session-dashboard__icon-button"
              aria-label={t('sessionDashboard.openFullConversation')}
              onClick={() => onNavigate(href)}
            >
              <ExternalLink size={15} aria-hidden="true" />
            </button>
          ) : null}
        </div>
        <FloatingPanelDescription>
          {dashboardProjectTitle(selection.owner.project, t)}
          {selection.owner.task?.title
            ? ` · ${selection.owner.task.title}`
            : ''}
        </FloatingPanelDescription>
        <RunReference runId={target.agent_run_id} />
      </FloatingPanelHeader>
      <WorkspaceSessionConversation
        workspaceId={target.workspace_id}
        sessionId={target.session_id}
        readOnly={selection.readOnly}
        viewState={view.conversation}
        onViewStateChange={(conversation) => updateView({ conversation })}
        previewContent={preview}
        onOpenWorkspaceFilePreview={(file) =>
          updateView({
            preview: {
              kind: 'workspace',
              workspaceId: file.workspaceId,
              repoId: file.repoId,
              path: file.path,
            },
          })
        }
      />
    </FloatingPanel>
  );
}
