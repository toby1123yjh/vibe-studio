import { create, useModal } from '@ebay/nice-modal-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { Button } from '@vibe/ui/components/Button';
import { Input } from '@vibe/ui/components/Input';
import { Textarea } from '@vibe/ui/components/Textarea';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/KeyboardDialog';
import { AgentProviderCapability, BaseCodingAgent } from 'shared/types';
import type { Repo } from 'shared/types';
import { repoApi } from '@/shared/lib/api';
import {
  arenaApi,
  type ArenaMode,
  type CreateArenaRequest,
} from '@/shared/lib/arenaApi';
import { arenaQueryKeys } from '@/shared/hooks/useArenaGroup';
import { useRepoBranches } from '@/shared/hooks/useRepoBranches';
import { useAgentProviderOptions } from '@/shared/hooks/useAgentProviderPolicy';
import { getValidProjectWorkspaceDefault } from '@/shared/hooks/useProjectRepoDefaults';
import BranchSelector from '@/shared/components/tasks/BranchSelector';
import { defineModal } from '@/shared/lib/modals';

interface CreateArenaDialogProps {
  projectId: string;
  taskId: string;
  hostId?: string | null;
  /** Suggested initial prompt (e.g. issue title + description). */
  initialPrompt?: string;
  /** Hard ceiling for attempts (project-level config; defaults to 6). */
  maxAttempts?: number;
}

export type CreateArenaDialogResult =
  | { kind: 'created'; groupId: string }
  | { kind: 'canceled' };

interface AttemptDraft {
  id: string;
  executor: BaseCodingAgent;
  variant: string;
}

const ARENA_MIN_ATTEMPTS = 2;
const ARENA_MAX_ATTEMPTS_DEFAULT = 6;
const FALLBACK_EXECUTOR_OPTIONS = Object.values(
  BaseCodingAgent
) as BaseCodingAgent[];
const DEFAULT_EXECUTOR = FALLBACK_EXECUTOR_OPTIONS[0] as BaseCodingAgent;
const ARENA_REQUIRED_CAPABILITIES = [
  AgentProviderCapability.INITIAL_RUN,
] as const;

let attemptIdCounter = 0;
function nextAttemptId(): string {
  attemptIdCounter += 1;
  return `att-${attemptIdCounter}`;
}

function defaultDraft(executor: BaseCodingAgent): AttemptDraft {
  return {
    id: nextAttemptId(),
    executor,
    variant: '',
  };
}

function getDefaultArenaExecutor(
  index: number,
  executors: readonly BaseCodingAgent[]
): BaseCodingAgent {
  return (
    executors[index] ??
    executors[0] ??
    FALLBACK_EXECUTOR_OPTIONS[index] ??
    DEFAULT_EXECUTOR
  );
}

function getRepoDisplayName(repo: Repo): string {
  return repo.display_name || repo.name;
}

function getRepoOptionLabel(repo: Repo): string {
  return `${getRepoDisplayName(repo)} - ${formatPathForDisplay(repo.path)}`;
}

function isActiveArenaConflict(errorMessage: string): boolean {
  return errorMessage.includes('already has an active arena group');
}

const CreateArenaDialogImpl = create<CreateArenaDialogProps>(
  ({
    projectId,
    taskId,
    hostId,
    initialPrompt = '',
    maxAttempts: maxAttemptsProp,
  }) => {
    const modal = useModal();
    const { t } = useTranslation('common');
    const queryClient = useQueryClient();
    const maxAttempts = Math.max(
      ARENA_MIN_ATTEMPTS,
      Math.min(
        maxAttemptsProp ?? ARENA_MAX_ATTEMPTS_DEFAULT,
        ARENA_MAX_ATTEMPTS_DEFAULT
      )
    );

    const [prompt, setPrompt] = useState(initialPrompt);
    const [mode, setMode] = useState<ArenaMode>('design');
    const [baseBranch, setBaseBranch] = useState('main');
    const [repoId, setRepoId] = useState<string | null>(null);
    const [attempts, setAttempts] = useState<AttemptDraft[]>(() => [
      defaultDraft(getDefaultArenaExecutor(0, FALLBACK_EXECUTOR_OPTIONS)),
      defaultDraft(getDefaultArenaExecutor(1, FALLBACK_EXECUTOR_OPTIONS)),
    ]);
    const [submitting, setSubmitting] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const canceledRef = useRef(false);

    const { options: arenaExecutorOptions, garage: agentGarage } =
      useAgentProviderOptions({
        fallbackExecutors: FALLBACK_EXECUTOR_OPTIONS,
        requiredCapabilities: ARENA_REQUIRED_CAPABILITIES,
      });
    const enabledArenaExecutors = useMemo(
      () =>
        arenaExecutorOptions
          .filter((option) => option.enabled)
          .map((option) => option.executor),
      [arenaExecutorOptions]
    );
    const arenaExecutorOptionByExecutor = useMemo(
      () =>
        new Map(
          arenaExecutorOptions.map((option) => [option.executor, option])
        ),
      [arenaExecutorOptions]
    );
    const getProviderDisabledLabel = useCallback(
      (reason: string | null | undefined): string => {
        if (reason === 'provider_capability_missing') {
          return t('agentProvider.disabled.capabilityMissing', {
            defaultValue: 'Missing required capability',
          });
        }
        return t('agentProvider.disabled.notReady', {
          defaultValue: 'Provider not ready',
        });
      },
      [t]
    );

    // Discover repos available for this project. We don't currently
    // filter by project membership — the user is expected to pick the
    // repo whose worktree should host these attempts.
    const { data: repos, isLoading: reposLoading } = useQuery<Repo[]>({
      queryKey: ['repoApi.list', hostId ?? 'local'],
      queryFn: () => repoApi.list(hostId),
      staleTime: 60_000,
    });

    const repoIdsKey = useMemo(
      () => repos?.map((repo) => repo.id).join('|') ?? '',
      [repos]
    );
    const { data: projectWorkspaceDefault = null, isLoading: defaultsLoading } =
      useQuery({
        queryKey: [
          'projectRepoDefaults',
          projectId,
          hostId ?? 'local',
          repoIdsKey,
        ],
        queryFn: () =>
          getValidProjectWorkspaceDefault(
            projectId,
            new Set((repos ?? []).map((repo) => repo.id)),
            hostId
          ),
        enabled: !!projectId && !!repos?.length,
        staleTime: 60_000,
      });

    const selectedRepo = useMemo(
      () => repos?.find((repo) => repo.id === repoId) ?? null,
      [repos, repoId]
    );
    const selectedProjectDefault = useMemo(
      () =>
        selectedRepo &&
        projectWorkspaceDefault?.kind === 'git' &&
        projectWorkspaceDefault.repo.repo_id === selectedRepo.id
          ? projectWorkspaceDefault.repo
          : undefined,
      [projectWorkspaceDefault, selectedRepo]
    );

    const {
      data: branches = [],
      isLoading: branchesLoading,
      isError: branchesError,
    } = useRepoBranches(repoId, {
      enabled: !!repoId,
    });

    useEffect(() => {
      if (repoId || !repos || repos.length === 0 || defaultsLoading) {
        return;
      }

      const preferredRepoId =
        projectWorkspaceDefault?.kind === 'git'
          ? projectWorkspaceDefault.repo.repo_id
          : undefined;
      const nextRepo =
        repos.find((repo) => repo.id === preferredRepoId) ?? repos[0];
      const preferredBranch =
        (projectWorkspaceDefault?.kind === 'git' &&
        projectWorkspaceDefault.repo.repo_id === nextRepo.id
          ? projectWorkspaceDefault.repo.target_branch
          : null) || nextRepo.default_target_branch;

      setRepoId(nextRepo.id);
      if (preferredBranch) {
        setBaseBranch(preferredBranch);
      }
    }, [repos, repoId, defaultsLoading, projectWorkspaceDefault]);

    useEffect(() => {
      if (!selectedRepo) return;

      const preferredBranch =
        selectedProjectDefault?.target_branch ||
        selectedRepo.default_target_branch;
      const currentBranch = branches.find((branch) => branch.is_current)?.name;
      const fallbackBranch =
        preferredBranch || currentBranch || branches[0]?.name;

      if (!baseBranch.trim() && fallbackBranch) {
        setBaseBranch(fallbackBranch);
        return;
      }

      if (
        branches.length > 0 &&
        !branches.some((branch) => branch.name === baseBranch) &&
        fallbackBranch
      ) {
        setBaseBranch(fallbackBranch);
      }
    }, [baseBranch, branches, selectedProjectDefault, selectedRepo]);

    useEffect(() => {
      if (enabledArenaExecutors.length === 0) return;

      setAttempts((prev) => {
        let changed = false;
        const next = prev.map((attempt, index) => {
          const option = arenaExecutorOptionByExecutor.get(attempt.executor);
          const shouldReplace =
            option?.enabled === false || (agentGarage !== null && !option);

          if (!shouldReplace) return attempt;

          changed = true;
          return {
            ...attempt,
            executor: getDefaultArenaExecutor(index, enabledArenaExecutors),
          };
        });

        return changed ? next : prev;
      });
    }, [agentGarage, arenaExecutorOptionByExecutor, enabledArenaExecutors]);

    const canAddAttempt =
      attempts.length < maxAttempts && enabledArenaExecutors.length > 0;
    const canRemoveAttempt = attempts.length > ARENA_MIN_ATTEMPTS;

    const handleClose = () => {
      canceledRef.current = true;
      modal.resolve({ kind: 'canceled' } satisfies CreateArenaDialogResult);
      modal.hide();
    };

    const handleOpenChange = (open: boolean) => {
      if (!open) {
        handleClose();
      }
    };

    const handleRepoChange = (nextRepoId: string) => {
      const nextRepo = repos?.find((repo) => repo.id === nextRepoId);
      const preferredBranch =
        (projectWorkspaceDefault?.kind === 'git' &&
        projectWorkspaceDefault.repo.repo_id === nextRepoId
          ? projectWorkspaceDefault.repo.target_branch
          : null) || nextRepo?.default_target_branch;

      setRepoId(nextRepoId || null);
      setBaseBranch(preferredBranch ?? '');
      setError(null);
    };

    const handleAddAttempt = () => {
      if (!canAddAttempt) return;
      setAttempts((prev) => [
        ...prev,
        defaultDraft(
          getDefaultArenaExecutor(prev.length, enabledArenaExecutors)
        ),
      ]);
    };

    const handleRemoveAttempt = (id: string) => {
      if (!canRemoveAttempt) return;
      setAttempts((prev) => prev.filter((a) => a.id !== id));
    };

    const handleAttemptChange = (
      id: string,
      patch: Partial<Omit<AttemptDraft, 'id'>>
    ) => {
      setAttempts((prev) =>
        prev.map((a) => (a.id === id ? { ...a, ...patch } : a))
      );
    };

    const validationError = useMemo<string | null>(() => {
      if (!prompt.trim()) return t('arena.create.validation.promptRequired');
      if (!baseBranch.trim())
        return t('arena.create.validation.baseBranchRequired');
      if (!repoId) return t('arena.create.validation.repositoryRequired');
      if (arenaExecutorOptions.length === 0) {
        return t('arena.create.validation.noAvailableAgents', {
          defaultValue: 'No agents are available for arena runs.',
        });
      }
      const unavailableAttempt = attempts.find((attempt) => {
        const option = arenaExecutorOptionByExecutor.get(attempt.executor);
        return option?.enabled === false || (agentGarage !== null && !option);
      });
      if (unavailableAttempt) {
        const option = arenaExecutorOptionByExecutor.get(
          unavailableAttempt.executor
        );
        const reason = getProviderDisabledLabel(option?.disabledReason);
        return t('arena.create.validation.executorUnavailable', {
          agent: unavailableAttempt.executor,
          reason,
          defaultValue: `${unavailableAttempt.executor} is unavailable for arena runs: ${reason}.`,
        });
      }
      if (attempts.length < ARENA_MIN_ATTEMPTS)
        return t('arena.create.validation.minAttempts', {
          count: ARENA_MIN_ATTEMPTS,
        });
      if (attempts.length > maxAttempts)
        return t('arena.create.validation.maxAttempts', {
          count: maxAttempts,
        });
      return null;
    }, [
      agentGarage,
      arenaExecutorOptionByExecutor,
      arenaExecutorOptions.length,
      attempts,
      baseBranch,
      getProviderDisabledLabel,
      maxAttempts,
      prompt,
      repoId,
      t,
    ]);

    const handleSubmit = async () => {
      if (validationError) {
        setError(validationError);
        return;
      }
      if (!repoId) return;
      canceledRef.current = false;
      setSubmitting(true);
      setError(null);

      const payload: CreateArenaRequest = {
        project_id: projectId,
        base_branch: baseBranch.trim(),
        prompt: prompt.trim(),
        mode,
        repos: [{ repo_id: repoId, target_branch: baseBranch.trim() }],
        attempts: attempts.map((attempt) => ({
          executor_config: {
            executor: attempt.executor,
            variant: attempt.variant.trim() || null,
            model_id: null,
            agent_id: null,
            reasoning_id: null,
            permission_policy: null,
          },
          name: null,
          prompt: null,
        })),
      };

      try {
        const group = await arenaApi.create(taskId, payload);
        if (canceledRef.current) return;
        queryClient.setQueryData(arenaQueryKeys.group(group.id), group);
        queryClient.setQueryData(arenaQueryKeys.activeForTask(taskId), group);
        modal.resolve({
          kind: 'created',
          groupId: group.id,
        } satisfies CreateArenaDialogResult);
        modal.hide();
      } catch (err) {
        const message =
          err instanceof Error ? err.message : t('arena.errors.createFailed');
        if (isActiveArenaConflict(message)) {
          const activeGroup = await arenaApi
            .getActiveForTask(taskId)
            .catch(() => null);
          if (canceledRef.current) return;
          if (activeGroup) {
            if (activeGroup.workspaces.length === 0) {
              await arenaApi.dissolve(activeGroup.id).catch(() => null);
              await queryClient.invalidateQueries({
                queryKey: arenaQueryKeys.activeForTask(taskId),
              });
              setError(t('arena.errors.emptyGroupRecovered'));
              return;
            }

            queryClient.setQueryData(
              arenaQueryKeys.group(activeGroup.id),
              activeGroup
            );
            queryClient.setQueryData(
              arenaQueryKeys.activeForTask(taskId),
              activeGroup
            );
            modal.resolve({
              kind: 'created',
              groupId: activeGroup.id,
            } satisfies CreateArenaDialogResult);
            modal.hide();
            return;
          }
        }

        if (canceledRef.current) return;
        setError(message);
        setSubmitting(false);
      }
    };

    const dialogTitle =
      mode === 'design'
        ? t('arena.create.titleDesign', { count: attempts.length })
        : t('arena.create.titleImplementation', { count: attempts.length });
    const dialogDescription =
      mode === 'design'
        ? t('arena.create.descriptionDesign')
        : t('arena.create.descriptionImplementation');

    return (
      <Dialog open={modal.visible} onOpenChange={handleOpenChange}>
        <DialogContent className="flex max-h-[min(720px,calc(100vh-2rem))] flex-col sm:max-w-[680px]">
          <DialogHeader>
            <DialogTitle>{dialogTitle}</DialogTitle>
            <DialogDescription>{dialogDescription}</DialogDescription>
          </DialogHeader>

          <div className="min-h-0 flex-1 space-y-base overflow-y-auto py-half pr-1">
            <div className="space-y-half">
              <label
                htmlFor="arena-prompt"
                className="text-xs font-medium text-low"
              >
                {t('arena.create.prompt')}
              </label>
              <Textarea
                id="arena-prompt"
                value={prompt}
                onChange={(e) => setPrompt(e.target.value)}
                placeholder={t('arena.create.promptPlaceholder')}
                rows={4}
                className="font-ibm-plex-mono"
              />
            </div>

            <div className="space-y-half">
              <span className="text-xs font-medium text-low">
                {t('arena.create.mode')}
              </span>
              <div className="inline-flex rounded border border-zinc-200 bg-secondary p-0.5 dark:border-zinc-800">
                <button
                  type="button"
                  className={`rounded px-3 py-1.5 text-xs ${
                    mode === 'design'
                      ? 'bg-primary text-high shadow-sm'
                      : 'text-low hover:text-normal'
                  }`}
                  aria-pressed={mode === 'design'}
                  onClick={() => setMode('design')}
                >
                  {t('arena.create.modeDesign')}
                </button>
                <button
                  type="button"
                  className={`rounded px-3 py-1.5 text-xs ${
                    mode === 'implementation'
                      ? 'bg-primary text-high shadow-sm'
                      : 'text-low hover:text-normal'
                  }`}
                  aria-pressed={mode === 'implementation'}
                  onClick={() => setMode('implementation')}
                >
                  {t('arena.create.modeImplementation')}
                </button>
              </div>
            </div>

            <div className="grid gap-half sm:grid-cols-2">
              <div className="space-y-half">
                <label
                  htmlFor="arena-repo"
                  className="text-xs font-medium text-low"
                >
                  {t('arena.create.repository')}
                </label>
                <select
                  id="arena-repo"
                  value={repoId ?? ''}
                  onChange={(e) => handleRepoChange(e.target.value)}
                  disabled={reposLoading}
                  className="h-10 w-full rounded border bg-secondary px-2 text-sm"
                >
                  {!repos || repos.length === 0 ? (
                    <option value="">
                      {reposLoading
                        ? t('arena.create.loadingRepositories')
                        : t('arena.create.noRepositories')}
                    </option>
                  ) : (
                    repos.map((repo) => (
                      <option key={repo.id} value={repo.id}>
                        {getRepoOptionLabel(repo)}
                      </option>
                    ))
                  )}
                </select>
                {selectedRepo ? (
                  <p
                    className="truncate text-[11px] text-low"
                    title={formatPathForDisplay(selectedRepo.path)}
                  >
                    {t('arena.create.loadedFrom', {
                      path: formatPathForDisplay(selectedRepo.path),
                    })}
                  </p>
                ) : null}
              </div>

              <div className="space-y-half">
                <label className="text-xs font-medium text-low">
                  {t('arena.create.baseBranch')}
                </label>
                <BranchSelector
                  branches={branches}
                  selectedBranch={baseBranch || null}
                  onBranchSelect={(branch) => {
                    setBaseBranch(branch);
                    setError(null);
                  }}
                  placeholder={
                    branchesLoading
                      ? t('arena.create.loadingBranches')
                      : branchesError
                        ? t('arena.create.failedBranches')
                        : t('arena.create.selectBranch')
                  }
                />
              </div>
            </div>

            <div className="space-y-half">
              <div className="flex items-center justify-between">
                <span className="text-xs font-medium text-low">
                  {t('arena.create.attempts')}
                </span>
                <Button
                  size="xs"
                  variant="outline"
                  type="button"
                  disabled={!canAddAttempt}
                  onClick={handleAddAttempt}
                >
                  {t('arena.actions.addAttempt', {
                    count: attempts.length,
                    max: maxAttempts,
                  })}
                </Button>
              </div>

              <ul className="space-y-half">
                {attempts.map((attempt, idx) => (
                  <li
                    key={attempt.id}
                    className="grid grid-cols-[auto_minmax(0,1fr)_minmax(0,1fr)_auto] items-center gap-half rounded-sm border border-zinc-200 bg-secondary px-half py-half dark:border-zinc-800"
                  >
                    <span className="w-6 text-center text-xs text-low">
                      #{idx + 1}
                    </span>
                    <select
                      value={attempt.executor}
                      onChange={(e) =>
                        handleAttemptChange(attempt.id, {
                          executor: e.target.value as BaseCodingAgent,
                        })
                      }
                      className="h-9 min-w-0 rounded border bg-primary px-2 text-sm"
                      aria-label={t('arena.aria.attemptExecutor', {
                        index: idx + 1,
                      })}
                      disabled={arenaExecutorOptions.length === 0}
                    >
                      {arenaExecutorOptions.length === 0 ? (
                        <option value={attempt.executor}>
                          {t('arena.create.noAvailableAgents', {
                            defaultValue: 'No available agents',
                          })}
                        </option>
                      ) : (
                        arenaExecutorOptions.map((option) => {
                          const disabledLabel = option.enabled
                            ? null
                            : getProviderDisabledLabel(option.disabledReason);
                          return (
                            <option
                              key={option.executor}
                              value={option.executor}
                              disabled={!option.enabled}
                              title={disabledLabel ?? undefined}
                            >
                              {disabledLabel
                                ? `${option.executor} - ${disabledLabel}`
                                : option.executor}
                            </option>
                          );
                        })
                      )}
                    </select>
                    <Input
                      value={attempt.variant}
                      onChange={(e) =>
                        handleAttemptChange(attempt.id, {
                          variant: e.target.value,
                        })
                      }
                      placeholder={t('arena.create.variantPlaceholder')}
                      className="min-w-0"
                      aria-label={t('arena.aria.attemptVariant', {
                        index: idx + 1,
                      })}
                    />
                    <Button
                      size="xs"
                      variant="ghost"
                      type="button"
                      onClick={() => handleRemoveAttempt(attempt.id)}
                      disabled={!canRemoveAttempt}
                      aria-label={t('arena.aria.removeAttempt', {
                        index: idx + 1,
                      })}
                    >
                      <X className="h-3.5 w-3.5" aria-hidden="true" />
                    </Button>
                  </li>
                ))}
              </ul>
            </div>

            {error ? (
              <p className="text-xs text-error" role="alert">
                {error}
              </p>
            ) : null}
          </div>

          <DialogFooter>
            <Button variant="outline" type="button" onClick={handleClose}>
              {t('buttons.cancel')}
            </Button>
            <Button
              type="button"
              onClick={() => void handleSubmit()}
              disabled={submitting || !!validationError}
            >
              {submitting
                ? t('arena.actions.starting')
                : t('arena.actions.startArena')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    );
  }
);

export const CreateArenaDialog = defineModal<
  CreateArenaDialogProps,
  CreateArenaDialogResult
>(CreateArenaDialogImpl);
