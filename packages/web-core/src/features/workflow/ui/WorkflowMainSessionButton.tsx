import { useRef, useState } from 'react';
import { useNavigate } from '@tanstack/react-router';
import { useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { Button } from '@vibe/ui/components/Button';
import { ApiError } from '@/shared/lib/api';
import { workflowManagementApi } from '@/shared/lib/workflowManagementApi';
import { buildWorkspaceSessionHref } from '@/shared/lib/routes/workspaceRoutes';
import { useAppShellProjects } from '@/shared/hooks/useAppShellProjects';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useHostId } from '@/shared/providers/HostIdProvider';
import type { WorkflowMainSessionView } from 'shared/types';

interface WorkflowMainSessionButtonProps {
  projectId: string;
  workflowId: string;
  taskId?: string;
  disabled?: boolean;
  beforePrepare?: () => Promise<boolean>;
  onPrepared?: (
    prepared: WorkflowMainSessionView,
    trigger: HTMLElement
  ) => void;
}

export function WorkflowMainSessionButton({
  projectId,
  workflowId,
  taskId,
  disabled,
  beforePrepare,
  onPrepared,
}: WorkflowMainSessionButtonProps) {
  const { t } = useTranslation('common');
  const navigate = useNavigate();
  const navigation = useAppNavigation();
  const shell = useAppShellProjects();
  const hostId = useHostId();
  const queryClient = useQueryClient();
  const scopeKey = JSON.stringify([hostId, projectId, workflowId, taskId]);
  const scopeRef = useRef({
    key: scopeKey,
    pending: false,
    requestId: null as string | null,
  });
  if (scopeRef.current.key !== scopeKey) {
    scopeRef.current = { key: scopeKey, pending: false, requestId: null };
  }
  const [state, setState] = useState({
    key: scopeKey,
    pending: false,
    error: null as string | null,
  });
  const pending = state.key === scopeKey && state.pending;
  const error = state.key === scopeKey ? state.error : null;
  const blockedReason =
    navigation.projectWorkflowUnavailableReason ??
    navigation.agentExecutionUnavailableReason;

  const handlePrepare = async (trigger: HTMLElement) => {
    const scope = scopeRef.current;
    if (scope.pending || disabled || blockedReason) return;
    scope.pending = true;
    setState({ key: scopeKey, pending: true, error: null });
    try {
      if (beforePrepare && !(await beforePrepare())) return;
      if (scopeRef.current !== scope) return;
      scope.requestId ??= crypto.randomUUID();
      const prepared = await workflowManagementApi.prepareMainSession(
        {
          project_id: projectId,
          workflow_id: workflowId,
          issue_id: taskId,
          request_id: scope.requestId,
        },
        hostId
      );
      if (scopeRef.current !== scope) return;
      const hostPrefix =
        shell?.deployment === 'remote' && shell.hostId
          ? `/hosts/${encodeURIComponent(shell.hostId)}`
          : '';
      const href = buildWorkspaceSessionHref(
        `${hostPrefix}/workspaces/${prepared.session.workspace_id}`,
        prepared.session.id
      );
      if (!href) throw new Error(t('workflow.management.sessionFailed'));
      void queryClient.invalidateQueries({
        queryKey: ['app-shell', 'discovery'],
      });
      void queryClient.invalidateQueries({ queryKey: ['project-sessions'] });
      if (onPrepared) {
        onPrepared(prepared, trigger);
      } else {
        await navigate({ to: href });
      }
    } catch (err) {
      if (scopeRef.current === scope) {
        let error = err instanceof Error ? err.message : String(err);
        const details: unknown =
          err instanceof ApiError ? err.error_data : undefined;
        if (
          details !== null &&
          typeof details === 'object' &&
          'message' in details &&
          typeof details.message === 'string' &&
          details.message.trim()
        ) {
          error =
            'code' in details &&
            details.code === 'workflow_conflict' &&
            details.message ===
              "Configure this workflow's captured main Agent before opening its Session"
              ? t('workflow.management.mainAgentMissing')
              : details.message;
        }
        setState({
          key: scopeKey,
          pending: false,
          error,
        });
      }
    } finally {
      scope.pending = false;
      if (scopeRef.current === scope) {
        setState((current) => ({ ...current, pending: false }));
      }
    }
  };

  return (
    <div className="flex flex-col gap-1">
      <Button
        type="button"
        variant="outline"
        loading={pending}
        disabled={disabled || !!blockedReason}
        title={blockedReason}
        onClick={(event) => void handlePrepare(event.currentTarget)}
      >
        {t('workflow.management.openConversation')}
      </Button>
      {error ? (
        <p role="alert" className="max-w-md text-base text-error">
          {error}
        </p>
      ) : null}
    </div>
  );
}
