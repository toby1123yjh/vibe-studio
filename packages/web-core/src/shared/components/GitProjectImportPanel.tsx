import { useEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import type { GitImportJob, GitRemoteInspection } from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import { Input } from '@vibe/ui/components/Input';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import { gitImportApi } from '@/shared/lib/gitImportApi';
import { ApiError } from '@/shared/lib/api';
import { FolderPickerDialog } from '@/shared/dialogs/shared/FolderPickerDialog';
import {
  readGitImportRecovery,
  writeGitImportRecovery,
  clearGitImportRecovery,
  type GitImportRecovery,
} from '@/shared/lib/gitImportRecovery';
import type { WorkspaceTargetSelection } from '@/shared/dialogs/shared/WorkspaceTargetDialog';

const active = (job: GitImportJob | null) =>
  !!job && ['queued', 'running', 'cancelling'].includes(job.state);
const fieldClass =
  'min-h-9 w-full rounded border border-border bg-primary px-2 text-sm text-normal focus-visible:ring-2 focus-visible:ring-brand';

export function GitProjectImportPanel({
  hostId,
  recoveryScope,
  enabled,
  onReady,
  onBusyChange,
}: {
  hostId: string | null;
  recoveryScope: string;
  enabled: boolean;
  onReady(selection: WorkspaceTargetSelection, jobId: string): void;
  onBusyChange(busy: boolean): void;
}) {
  const { t } = useTranslation('projects');
  const [initial] = useState(() => {
    try {
      return {
        record: readGitImportRecovery(hostId, recoveryScope),
        failed: false,
      };
    } catch {
      return { record: null, failed: true };
    }
  });
  const recovery = useRef<GitImportRecovery | null>(initial.record);
  const [recoveryFailed, setRecoveryFailed] = useState(initial.failed);
  const [restoring, setRestoring] = useState(!!initial.record?.jobId);
  const [restoreRetry, setRestoreRetry] = useState(0);
  const [url, setUrl] = useState(initial.record?.request.url ?? '');
  const [connectionId, setConnectionId] = useState(
    initial.record?.request.connection_id ?? ''
  );
  const [inspection, setInspection] = useState<GitRemoteInspection | null>(
    initial.record?.inspection ?? null
  );
  const [branch, setBranch] = useState(initial.record?.request.branch ?? '');
  const [directory, setDirectory] = useState(
    initial.record?.request.directory_path ?? ''
  );
  const [job, setJob] = useState<GitImportJob | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pollRetry, setPollRetry] = useState(0);
  const [pollFailed, setPollFailed] = useState(false);
  const [startUncertain, setStartUncertain] = useState(
    !!initial.record && !initial.record.jobId
  );
  const jobRef = useRef<GitImportJob | null>(null);
  const deliveredJob = useRef<string | null>(null);
  const lock = useRef(false);
  const requestId = useRef<string | null>(
    initial.record?.request.request_id ?? null
  );
  const generation = useRef(0);
  const latest = useRef({ hostId, enabled, onReady, onBusyChange });
  if (latest.current.hostId !== hostId) generation.current += 1;
  latest.current = { hostId, enabled, onReady, onBusyChange };
  useEffect(
    () => () => {
      generation.current += 1;
    },
    []
  );
  const connections = useQuery({
    queryKey: ['git-connections', hostId],
    queryFn: () => gitImportApi.connections(hostId),
    enabled,
  });
  const busy = pending || active(job) || startUncertain || restoring;
  useEffect(() => {
    if (!busy) return;
    const guard = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', guard);
    return () => window.removeEventListener('beforeunload', guard);
  }, [busy]);
  useEffect(() => {
    onBusyChange(busy);
    return () => onBusyChange(false);
  }, [busy, onBusyChange]);
  const publish = (next: GitImportJob) => {
    if (jobRef.current?.id === next.id && !active(jobRef.current)) return;
    jobRef.current = next;
    setJob((previous) =>
      previous && !active(previous) && previous.id === next.id ? previous : next
    );
  };
  useEffect(() => {
    if (
      enabled &&
      job?.state === 'succeeded' &&
      job.repo &&
      deliveredJob.current !== job.id
    ) {
      deliveredJob.current = job.id;
      latest.current.onReady(
        job.branch
          ? {
              mode: 'worktree',
              path: job.directory_path,
              repo: job.repo,
              targetBranch: job.branch,
            }
          : { mode: 'direct_folder', path: job.directory_path },
        job.id
      );
    }
  }, [job, enabled]);
  const publishRef = useRef(publish);
  publishRef.current = publish;
  useEffect(() => {
    const saved = recovery.current;
    if (!saved?.jobId || !enabled || jobRef.current) return;
    let disposed = false;
    setRestoring(true);
    void gitImportApi
      .get(hostId, saved.jobId)
      .then((next) => {
        if (disposed) return;
        publishRef.current(next);
        setRestoring(false);
        setRecoveryFailed(false);
      })
      .catch(() => {
        if (!disposed) {
          setRestoring(false);
          setRecoveryFailed(true);
        }
      });
    return () => {
      disposed = true;
    };
  }, [initial, hostId, enabled, restoreRetry]);
  const jobId = job?.id;
  const jobState = job?.state;
  useEffect(() => {
    if (
      !jobId ||
      !jobState ||
      !['queued', 'running', 'cancelling'].includes(jobState) ||
      !enabled
    )
      return;
    const epoch = generation.current;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const next = await gitImportApi.get(hostId, jobId);
        if (disposed || generation.current !== epoch) return;
        setPollFailed(false);
        publishRef.current(next);
        if (active(next)) timer = setTimeout(() => void poll(), 500);
      } catch (cause) {
        if (disposed || generation.current !== epoch) return;
        setPollFailed(true);
        setError(
          cause instanceof Error
            ? cause.message
            : t('gitImport.failed', 'Git operation failed. Try again.')
        );
      }
    };
    timer = setTimeout(() => void poll(), 500);
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [jobId, jobState, enabled, hostId, pollRetry, t]);
  const perform = async (operation: () => Promise<void>) => {
    if (lock.current || !latest.current.enabled) return;
    lock.current = true;
    setPending(true);
    setError(null);
    const epoch = generation.current;
    try {
      await operation();
    } catch (cause) {
      if (generation.current === epoch)
        setError(
          cause instanceof Error
            ? cause.message
            : t('gitImport.failed', 'Git operation failed. Try again.')
        );
    } finally {
      lock.current = false;
      if (generation.current === epoch) setPending(false);
    }
  };
  const inspect = () =>
    perform(async () => {
      const epoch = generation.current;
      const result = await gitImportApi.inspect(
        hostId,
        url.trim(),
        connectionId || null
      );
      if (generation.current !== epoch || !latest.current.enabled) return;
      setInspection(result);
      setBranch(result.default_branch ?? '');
    });
  const clone = () =>
    perform(async () => {
      if (!inspection || active(job)) return;
      const epoch = generation.current;
      requestId.current ??= crypto.randomUUID();
      const saved = recovery.current ?? {
        request: {
          request_id: requestId.current,
          url: inspection.url,
          connection_id: connectionId || null,
          branch: branch || null,
          directory_path: directory.trim() || null,
        },
        inspection,
        jobId: null,
      };
      // Persist before POST so a lost response cannot lead to a second clone.
      writeGitImportRecovery(hostId, recoveryScope, saved);
      recovery.current = saved;
      setStartUncertain(true);
      let result: GitImportJob;
      try {
        result = await gitImportApi.start(hostId, saved.request);
      } catch (cause) {
        if (
          generation.current === epoch &&
          cause instanceof ApiError &&
          cause.status !== undefined &&
          cause.status >= 400 &&
          cause.status < 500
        ) {
          setStartUncertain(false);
          clearGitImportRecovery(hostId, recoveryScope);
          recovery.current = null;
        }
        throw cause;
      }
      if (generation.current !== epoch) return;
      setStartUncertain(false);
      recovery.current = { ...saved, jobId: result.id };
      try {
        writeGitImportRecovery(hostId, recoveryScope, recovery.current);
      } catch {
        setRecoveryFailed(true);
      }
      publish(result);
    });
  const cancel = () =>
    perform(async () => {
      if (!job || !active(job)) return;
      const epoch = generation.current;
      const result = await gitImportApi.cancel(hostId, job.id);
      if (generation.current === epoch) publish(result);
    });
  const resetInspection = () => {
    clearGitImportRecovery(hostId, recoveryScope);
    recovery.current = null;
    setInspection(null);
    setJob(null);
    jobRef.current = null;
    requestId.current = null;
    setError(null);
  };
  const terminalFailure = job?.state === 'failed' || job?.state === 'cancelled';
  const locked =
    busy || recoveryFailed || job?.state === 'succeeded' || !enabled;
  return (
    <div className="space-y-3">
      <label className="block space-y-1 text-sm">
        {t('gitImport.url', 'Repository URL')}
        <Input
          value={url}
          disabled={locked}
          onChange={(e) => {
            setUrl(e.target.value);
            resetInspection();
          }}
          placeholder="git@github.com:owner/repository.git"
        />
      </label>
      <label className="block space-y-1 text-sm">
        {t('gitImport.connection', 'SSH connection')}
        <select
          className={fieldClass}
          value={connectionId}
          disabled={locked}
          onChange={(e) => {
            setConnectionId(e.target.value);
            resetInspection();
          }}
        >
          <option value="">
            {t(
              'gitImport.native',
              'Use this machine’s Git / SSH configuration'
            )}
          </option>
          {(connections.data ?? []).map((connection) => (
            <option
              key={connection.id}
              value={connection.id}
              disabled={!connection.credential_ready}
              title={connection.credential_error ?? undefined}
            >
              {connection.name} — {connection.host}
            </option>
          ))}
        </select>
      </label>
      {connections.isError && (
        <p role="alert" className="text-xs text-error">
          {t(
            'gitImport.connectionsFailed',
            'Connections could not be loaded. Native Git remains available.'
          )}{' '}
          <Button variant="ghost" onClick={() => void connections.refetch()}>
            {t('common:buttons.retry', 'Retry')}
          </Button>
        </p>
      )}
      {!inspection && (
        <Button
          type="button"
          variant="outline"
          disabled={locked || !url.trim()}
          onClick={() => void inspect()}
        >
          {pending
            ? t('gitImport.inspecting', 'Reading branches…')
            : t('gitImport.inspect', 'Read branches')}
        </Button>
      )}
      {inspection && (
        <>
          <label className="block space-y-1 text-sm">
            {t('gitImport.branch', 'Branch')}
            <select
              className={fieldClass}
              value={branch}
              disabled={locked}
              onChange={(e) => {
                setBranch(e.target.value);
                requestId.current = null;
              }}
            >
              <option value="">
                {t('gitImport.defaultBranch', 'Repository default')}
              </option>
              {inspection.branches.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </select>
          </label>
          <Button
            type="button"
            variant="outline"
            disabled={locked}
            onClick={() =>
              void perform(async () => {
                const epoch = generation.current;
                const parent = await FolderPickerDialog.show({
                  hostId,
                  title: t('gitImport.browseParent', 'Choose parent folder'),
                });
                if (
                  !parent ||
                  generation.current !== epoch ||
                  !latest.current.enabled
                )
                  return;
                const child = inspection.suggested_directory
                  .split(/[\\/]/)
                  .filter(Boolean)
                  .pop();
                if (!child) return;
                const separator = parent.includes('\\') ? '\\' : '/';
                setDirectory(
                  `${parent.replace(/[\\/]+$/, '')}${separator}${child}`
                );
                requestId.current = null;
              })
            }
          >
            {t('gitImport.browseParent', 'Choose parent folder')}
          </Button>
          <label className="block space-y-1 text-sm">
            {t('gitImport.directory', 'Download directory')}
            <Input
              value={formatPathForDisplay(directory)}
              disabled={locked}
              placeholder={formatPathForDisplay(inspection.suggested_directory)}
              onChange={(e) => {
                setDirectory(e.target.value);
                requestId.current = null;
              }}
            />
          </label>
          <p className="text-xs text-low">
            {t(
              'gitImport.directoryHint',
              'Leave blank to create a new directory automatically. Existing directories are never overwritten.'
            )}
          </p>
          {!job && (
            <Button
              type="button"
              disabled={pending || restoring || recoveryFailed || !enabled}
              onClick={() => void clone()}
            >
              {pending
                ? t('gitImport.starting', 'Starting…')
                : startUncertain
                  ? t('gitImport.recoverStart', 'Recover download request')
                  : t('gitImport.clone', 'Download repository')}
            </Button>
          )}
        </>
      )}
      {job && (
        <div
          role="status"
          className="space-y-2 break-words rounded border border-border p-3 text-sm"
        >
          <p>
            {t(`gitImport.state.${job.state}`, { defaultValue: job.state })}
          </p>
          <p className="break-all text-xs text-low">
            {formatPathForDisplay(job.directory_path)}
          </p>
          {job.progress !== null && active(job) && (
            <progress
              className="w-full"
              max={100}
              value={job.progress}
              aria-label={t('gitImport.progress', 'Download progress')}
            />
          )}
          {active(job) && (
            <Button
              type="button"
              variant="outline"
              disabled={pending || job.state === 'cancelling' || !enabled}
              onClick={() => void cancel()}
            >
              {t('gitImport.cancel', 'Cancel download')}
            </Button>
          )}
          {terminalFailure && (
            <>
              <p>{job.error}</p>
              <p className="text-xs text-low">
                {t(
                  'gitImport.retained',
                  'Partial files are kept. Choose a new directory to retry, or leave it blank for an automatic directory.'
                )}
              </p>
              <Button
                type="button"
                disabled={pending || !enabled}
                onClick={() => {
                  clearGitImportRecovery(hostId, recoveryScope);
                  recovery.current = null;
                  setJob(null);
                  jobRef.current = null;
                  requestId.current = null;
                  setError(null);
                  setDirectory('');
                }}
              >
                {t('gitImport.retry', 'Prepare retry')}
              </Button>
            </>
          )}
        </div>
      )}
      {error && (
        <p role="alert" className="text-sm text-error">
          {error}
        </p>
      )}
      {recoveryFailed && (
        <div role="alert" className="space-y-2 text-sm text-error">
          <p>
            {t(
              'gitImport.recoveryFailed',
              'The saved download could not be restored. Retry before starting another download.'
            )}
          </p>
          {initial.record?.jobId && (
            <Button
              variant="outline"
              onClick={() => setRestoreRetry((v) => v + 1)}
            >
              {t('common:buttons.retry', 'Retry')}
            </Button>
          )}
        </div>
      )}
      {pollFailed && (
        <Button
          type="button"
          variant="outline"
          disabled={!enabled}
          onClick={() => {
            setError(null);
            setPollFailed(false);
            setPollRetry((v) => v + 1);
          }}
        >
          {t('gitImport.reconnect', 'Reconnect to download')}
        </Button>
      )}
    </div>
  );
}
