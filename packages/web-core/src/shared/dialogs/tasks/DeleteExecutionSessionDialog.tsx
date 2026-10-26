import { useEffect, useId, useRef, useState } from 'react';
import { create, useModal } from '@ebay/nice-modal-react';
import { useTranslation } from 'react-i18next';
import { LoaderCircle } from 'lucide-react';
import type {
  ExecutionSummary,
  SessionDeletionInfo,
  SessionDeletionResult,
} from 'shared/types';
import { ConfirmDialogView } from '@vibe/ui/components/ConfirmDialog';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import { defineModal, type DeleteResult } from '@/shared/lib/modals';
import { ApiError } from '@/shared/lib/api';
import './delete-task-session-dialog.css';

export interface ResolvedExecutionSessionDeleteTarget {
  title: string;
  execution: ExecutionSummary | null;
  deletionInfo?: SessionDeletionInfo;
}

interface DeleteExecutionSessionDialogProps {
  resolveTarget(): Promise<ResolvedExecutionSessionDeleteTarget>;
  onDelete(
    target: ResolvedExecutionSessionDeleteTarget,
    stopRunning: boolean,
    deleteManagedFiles: boolean
  ): Promise<SessionDeletionResult | void>;
}

const DeleteExecutionSessionDialogImpl =
  create<DeleteExecutionSessionDialogProps>(({ resolveTarget, onDelete }) => {
    const modal = useModal();
    const { t } = useTranslation(['tasks', 'common']);
    const pendingRef = useRef(false);
    const filesHintId = useId();
    const [target, setTarget] =
      useState<ResolvedExecutionSessionDeleteTarget | null>(null);
    const [loading, setLoading] = useState(true);
    const [deleting, setDeleting] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [retry, setRetry] = useState(0);
    const [requiresStop, setRequiresStop] = useState(false);
    const [deleteManagedFiles, setDeleteManagedFiles] = useState(false);
    const [completedWarning, setCompletedWarning] = useState<string | null>(
      null
    );

    useEffect(() => {
      let active = true;
      setTarget(null);
      setLoading(true);
      setError(null);
      setDeleteManagedFiles(false);
      void resolveTarget().then(
        (resolved) => {
          if (active) {
            setTarget(resolved);
            setLoading(false);
          }
        },
        (cause: unknown) => {
          if (active) {
            setError(
              cause instanceof Error
                ? cause.message
                : t('sessionDeletion.failed')
            );
            setLoading(false);
          }
        }
      );
      return () => {
        active = false;
      };
    }, [resolveTarget, retry, t]);

    const close = (result: DeleteResult) => {
      modal.resolve(result);
      void modal.hide();
      modal.remove();
    };
    const cancel = () => {
      if (!pendingRef.current) close(completedWarning ? 'deleted' : 'canceled');
    };
    const confirm = async () => {
      if (!target || loading || pendingRef.current) return;
      pendingRef.current = true;
      setDeleting(true);
      setError(null);
      try {
        const result = await onDelete(
          target,
          requiresStop,
          deleteManagedFiles &&
            target.deletionInfo?.can_delete_managed_files === true
        );
        if (result?.warning) {
          setCompletedWarning(result.warning);
          return;
        }
        close('deleted');
      } catch (cause) {
        if (
          cause instanceof ApiError &&
          cause.status === 409 &&
          typeof cause.error_data === 'object' &&
          cause.error_data !== null &&
          'code' in cause.error_data &&
          cause.error_data.code === 'session_requires_stop'
        ) {
          setRequiresStop(true);
          setError(
            requiresStop
              ? t('sessionDeletion.stopFailed', {
                  defaultValue:
                    'The agent has not stopped yet. Your execution and session were kept; retry when ready.',
                })
              : null
          );
          return;
        }
        setError(
          cause instanceof Error ? cause.message : t('sessionDeletion.failed')
        );
      } finally {
        pendingRef.current = false;
        setDeleting(false);
      }
    };

    const retryLookup = !target && !!error;
    return (
      <ConfirmDialogView
        className="vk-session-delete"
        open={modal.visible}
        variant="destructive"
        icon={false}
        restoreFocus={false}
        title={t(
          target?.execution
            ? 'sessionDeletion.taskTitle'
            : 'sessionDeletion.sessionTitle'
        )}
        message={
          completedWarning
            ? t('sessionDeletion.filesRetained', {
                defaultValue: 'Session deleted. Some files were kept:',
              })
            : requiresStop
              ? t('sessionDeletion.stopDescription', {
                  name: target?.title,
                  defaultValue:
                    'The agent for “{{name}}” is still running or its exit cannot be confirmed. Stop it safely, then delete? If stopping fails, the execution and session will be kept.',
                })
              : target
                ? t(
                    target.execution
                      ? 'sessionDeletion.taskSummary'
                      : 'sessionDeletion.sessionSummary',
                    {
                      defaultValue: target.execution
                        ? 'This execution and its session history will be deleted. This cannot be undone.'
                        : 'This session and its history will be deleted. This cannot be undone.',
                    }
                  )
                : t('sessionDeletion.resolving')
        }
        showCancelButton={!completedWarning}
        cancelText={t('common:buttons.cancel', 'Cancel')}
        confirmText={
          completedWarning
            ? t('common:buttons.close', 'Close')
            : requiresStop
              ? t(
                  deleting
                    ? 'sessionDeletion.stopping'
                    : 'sessionDeletion.stopAndDelete',
                  {
                    defaultValue: deleting
                      ? 'Stopping and deleting…'
                      : 'Stop and delete',
                  }
                )
              : retryLookup
                ? t('common:buttons.retry', 'Retry')
                : t(
                    deleting
                      ? 'sessionDeletion.deleting'
                      : target?.execution
                        ? 'sessionDeletion.deleteTask'
                        : 'sessionDeletion.deleteSession'
                  )
        }
        confirmVariant={
          retryLookup || completedWarning ? 'default' : 'destructive'
        }
        confirmDisabled={loading || (!target && !retryLookup)}
        confirmPending={deleting}
        cancelDisabled={deleting}
        onCancel={cancel}
        onConfirm={() => {
          if (completedWarning) close('deleted');
          else if (retryLookup) setRetry((value) => value + 1);
          else void confirm();
        }}
      >
        {target && !completedWarning && (
          <div className="vk-session-delete__target">{target.title}</div>
        )}
        {target?.deletionInfo?.can_delete_managed_files &&
          !completedWarning && (
            <section
              className="vk-session-delete__files"
              data-selected={deleteManagedFiles}
            >
              <label className="vk-session-delete__option">
                <input
                  type="checkbox"
                  checked={deleteManagedFiles}
                  disabled={deleting}
                  onChange={(event) =>
                    setDeleteManagedFiles(event.target.checked)
                  }
                  aria-describedby={filesHintId}
                />
                <span>
                  {t('sessionDeletion.deleteManagedFiles', {
                    defaultValue:
                      'Also delete this session’s working directory and files',
                  })}
                </span>
              </label>
              <p
                id={filesHintId}
                className="vk-session-delete__hint"
                aria-live="polite"
              >
                {deleteManagedFiles
                  ? t('sessionDeletion.filesWillDelete', {
                      defaultValue:
                        'The directory and all files inside will be permanently deleted.',
                    })
                  : t('sessionDeletion.filesWillKeep', {
                      defaultValue: 'Unchecked: files stay on this device.',
                    })}
              </p>
              <details className="vk-session-delete__path">
                <summary>
                  {t('sessionDeletion.directoryLabel', {
                    defaultValue: 'Working directory',
                  })}
                </summary>
                <code>
                  {formatPathForDisplay(
                    target.deletionInfo.managed_directory_path ?? ''
                  )}
                </code>
              </details>
            </section>
          )}
        {target && !completedWarning && (
          <p className="vk-session-delete__scope">
            {t('sessionDeletion.unaffected', {
              defaultValue:
                'Other sessions, projects, and native agent history are not affected.',
            })}
          </p>
        )}
        {completedWarning && (
          <p role="status" className="break-words text-sm text-normal">
            {completedWarning}
          </p>
        )}
        {loading && (
          <LoaderCircle
            aria-hidden="true"
            className="size-4 animate-spin motion-reduce:animate-none"
          />
        )}
        {error && (
          <p role="alert" className="text-sm text-[var(--vk-status-error)]">
            {error}
          </p>
        )}
      </ConfirmDialogView>
    );
  });

export const DeleteExecutionSessionDialog = defineModal<
  DeleteExecutionSessionDialogProps,
  DeleteResult
>(DeleteExecutionSessionDialogImpl);
