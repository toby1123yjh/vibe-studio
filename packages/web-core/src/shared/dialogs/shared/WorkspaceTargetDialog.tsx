import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { create, useModal } from '@ebay/nice-modal-react';
import {
  Folder,
  FolderOpen,
  GitBranch as GitBranchIcon,
  Loader2,
  Info,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { DirectoryInspection, GitBranch, Repo } from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import { Tooltip } from '@vibe/ui/components/Tooltip';
import { Input } from '@vibe/ui/components/Input';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/KeyboardDialog';
import BranchSelector from '@/shared/components/tasks/BranchSelector';
import { FolderPickerDialog } from '@/shared/dialogs/shared/FolderPickerDialog';
import { fileSystemApi, repoApi } from '@/shared/lib/api';
import { cn } from '@/shared/lib/utils';
import { defineModal } from '@/shared/lib/modals';

export type WorkspaceTargetMode = 'worktree' | 'direct_folder';
export type WorkspaceTargetDialogPurpose = 'execution' | 'project_default';

export type WorkspaceTargetSelection =
  | {
      mode: 'worktree';
      path: string;
      repo: Repo;
      targetBranch: string;
    }
  | {
      mode: 'direct_folder';
      path: string;
    };

export type WorkspaceTargetDialogResult =
  | { kind: 'confirmed'; selection: WorkspaceTargetSelection }
  | { kind: 'canceled' };

export interface WorkspaceTargetDialogProps {
  initialPath?: string;
  initialMode?: WorkspaceTargetMode;
  initialBranch?: string | null;
  hostId?: string | null;
  allowedModes?: WorkspaceTargetMode[];
  purpose?: WorkspaceTargetDialogPurpose;
  title?: string;
  description?: string;
}

type PendingAction = 'browse' | 'inspect' | 'branches' | null;
type DialogError = {
  scope: 'path' | 'branch';
  message: string;
};

const DEFAULT_ALLOWED_MODES: WorkspaceTargetMode[] = [
  'worktree',
  'direct_folder',
];

const WorkspaceTargetDialogImpl = create<WorkspaceTargetDialogProps>(
  ({
    initialPath = '',
    initialMode,
    initialBranch = null,
    hostId,
    allowedModes = DEFAULT_ALLOWED_MODES,
    purpose = 'execution',
    title,
    description,
  }) => {
    const modal = useModal();
    const { t } = useTranslation('common');
    const requestIdRef = useRef(0);
    const [path, setPath] = useState(initialPath);
    const [inspection, setInspection] = useState<DirectoryInspection | null>(
      null
    );
    const [mode, setMode] = useState<WorkspaceTargetMode | null>(
      initialMode ?? null
    );
    const [branches, setBranches] = useState<GitBranch[]>([]);
    const [targetBranch, setTargetBranch] = useState<string | null>(
      initialBranch
    );
    const [pendingAction, setPendingAction] = useState<PendingAction>(null);
    const [error, setError] = useState<DialogError | null>(null);
    const allowedModeSet = useMemo(() => new Set(allowedModes), [allowedModes]);
    const onlyAllowedMode = allowedModes.length === 1 ? allowedModes[0] : null;
    const isProjectDefault = purpose === 'project_default';

    const resetDerivedState = useCallback(() => {
      requestIdRef.current += 1;
      setInspection(null);
      setMode(null);
      setBranches([]);
      setTargetBranch(null);
      setError(null);
      setPendingAction(null);
    }, []);

    const loadBranches = useCallback(
      async (repo: Repo, preferredBranch?: string | null) => {
        const requestId = ++requestIdRef.current;
        setPendingAction('branches');
        setError(null);

        try {
          const nextBranches = await repoApi.getBranches(repo.id, hostId);
          if (requestId !== requestIdRef.current) return;

          setBranches(nextBranches);
          setTargetBranch(
            preferredBranch &&
              nextBranches.some((branch) => branch.name === preferredBranch)
              ? preferredBranch
              : null
          );
        } catch (nextError) {
          if (requestId !== requestIdRef.current) return;
          setBranches([]);
          setTargetBranch(null);
          setError({
            scope: 'branch',
            message:
              nextError instanceof Error
                ? nextError.message
                : t('createMode.workspaceDialog.errors.loadBranches', {
                    defaultValue: 'Failed to load repository branches.',
                  }),
          });
        } finally {
          if (requestId === requestIdRef.current) {
            setPendingAction(null);
          }
        }
      },
      [hostId, t]
    );

    const inspectPath = useCallback(
      async (
        nextPath: string,
        preferredMode?: WorkspaceTargetMode,
        preferredBranch?: string | null
      ) => {
        const trimmedPath = nextPath.trim();
        if (!trimmedPath) {
          resetDerivedState();
          setError({
            scope: 'path',
            message: t('createMode.workspaceDialog.errors.pathRequired', {
              defaultValue: 'Choose or enter a directory path.',
            }),
          });
          return;
        }

        const requestId = ++requestIdRef.current;
        setPendingAction('inspect');
        setError(null);
        setInspection(null);
        setMode(null);
        setBranches([]);
        setTargetBranch(null);

        try {
          const nextInspection = await fileSystemApi.inspectDirectory(
            { path: trimmedPath },
            hostId
          );
          if (requestId !== requestIdRef.current) return;

          setPath(nextInspection.path);
          setInspection(nextInspection);

          if (!nextInspection.is_git_repo || !nextInspection.repo) {
            const nextMode =
              isProjectDefault ||
              (allowedModeSet.has('direct_folder') &&
                (preferredMode === 'direct_folder' ||
                  onlyAllowedMode === 'direct_folder'))
                ? 'direct_folder'
                : null;
            setMode(nextMode);
            return;
          }

          const nextMode = isProjectDefault
            ? 'worktree'
            : preferredMode && allowedModeSet.has(preferredMode)
              ? preferredMode
              : onlyAllowedMode;
          setMode(nextMode);
          if (nextMode === 'worktree') {
            await loadBranches(nextInspection.repo, preferredBranch);
          }
        } catch (nextError) {
          if (requestId !== requestIdRef.current) return;
          setError({
            scope: 'path',
            message:
              nextError instanceof Error
                ? nextError.message
                : t('createMode.workspaceDialog.errors.inspect', {
                    defaultValue: 'Unable to inspect this directory.',
                  }),
          });
        } finally {
          if (requestId === requestIdRef.current) {
            setPendingAction(null);
          }
        }
      },
      [
        allowedModeSet,
        hostId,
        isProjectDefault,
        loadBranches,
        onlyAllowedMode,
        resetDerivedState,
        t,
      ]
    );

    useEffect(() => {
      if (!modal.visible) return;

      setPath(initialPath);
      setInspection(null);
      setMode(
        initialMode && allowedModeSet.has(initialMode)
          ? initialMode
          : onlyAllowedMode
      );
      setBranches([]);
      setTargetBranch(initialBranch);
      setError(null);
      setPendingAction(null);

      if (initialPath.trim()) {
        void inspectPath(initialPath, initialMode, initialBranch);
      }
    }, [
      allowedModeSet,
      initialBranch,
      initialMode,
      initialPath,
      inspectPath,
      modal.visible,
      onlyAllowedMode,
    ]);

    const handlePathChange = (nextPath: string) => {
      setPath(nextPath);
      resetDerivedState();
    };

    const handleBrowse = async () => {
      const requestId = ++requestIdRef.current;
      setPendingAction('browse');
      setError(null);

      try {
        const selectedPath = await FolderPickerDialog.show({
          value: path,
          title: t('createMode.workspaceDialog.nativePickerTitle', {
            defaultValue: 'Choose working directory',
          }),
          hostId,
        });
        if (requestId !== requestIdRef.current) return;
        if (!selectedPath) return;

        setPath(selectedPath);
        await inspectPath(selectedPath);
      } catch (nextError) {
        if (requestId !== requestIdRef.current) return;
        setError({
          scope: 'path',
          message:
            nextError instanceof Error
              ? nextError.message
              : t('createMode.workspaceDialog.errors.browse', {
                  defaultValue: 'Unable to open the folder picker.',
                }),
        });
      } finally {
        if (requestId === requestIdRef.current) {
          setPendingAction(null);
        }
      }
    };

    const handleModeChange = (nextMode: WorkspaceTargetMode) => {
      if (!allowedModeSet.has(nextMode)) return;
      const repo = inspection?.repo;
      if (nextMode === 'worktree' && !repo) return;

      if (nextMode === 'direct_folder') {
        requestIdRef.current += 1;
        setPendingAction(null);
        setMode(nextMode);
        setBranches([]);
        setTargetBranch(null);
        setError(null);
        return;
      }

      if (pendingAction === 'branches') return;
      if (!repo) return;
      setMode(nextMode);
      setBranches([]);
      setTargetBranch(null);
      setError(null);
      void loadBranches(repo);
    };

    const handleCancel = () => {
      requestIdRef.current += 1;
      modal.resolve({ kind: 'canceled' } satisfies WorkspaceTargetDialogResult);
      modal.hide();
    };

    const handleConfirm = () => {
      if (!inspection || !mode) return;

      if (mode === 'worktree') {
        if (!inspection.repo || !targetBranch) return;
        modal.resolve({
          kind: 'confirmed',
          selection: {
            mode,
            path: inspection.path,
            repo: inspection.repo,
            targetBranch,
          },
        } satisfies WorkspaceTargetDialogResult);
      } else {
        modal.resolve({
          kind: 'confirmed',
          selection: {
            mode,
            path: inspection.path,
          },
        } satisfies WorkspaceTargetDialogResult);
      }

      modal.hide();
    };

    const canConfirm =
      inspection !== null &&
      mode !== null &&
      allowedModeSet.has(mode) &&
      pendingAction === null &&
      (mode === 'direct_folder' ||
        (inspection.repo !== null && targetBranch !== null));
    const isBusy = pendingAction !== null;

    const confirmLabel = useMemo(
      () =>
        isProjectDefault
          ? t('createMode.workspaceDialog.useProjectLocation', {
              defaultValue: 'Use this location',
            })
          : mode === 'direct_folder'
            ? t('createMode.workspaceDialog.useDirectory', {
                defaultValue: 'Use this directory',
              })
            : t('createMode.workspaceDialog.useWorkspace', {
                defaultValue: 'Use this workspace',
              }),
      [isProjectDefault, mode, t]
    );

    return (
      <Dialog
        open={modal.visible}
        onOpenChange={(open) => {
          if (!open) handleCancel();
        }}
      >
        <DialogContent
          role="dialog"
          aria-modal="true"
          aria-labelledby="workspace-target-title"
          className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
          style={{ maxWidth: 'min(640px, calc(100vw - 32px))' }}
        >
          <DialogHeader>
            <DialogTitle id="workspace-target-title">
              {title ??
                t('createMode.workspaceDialog.title', {
                  defaultValue: 'Choose working directory',
                })}
            </DialogTitle>
            <DialogDescription className="sr-only">
              {description ??
                t('createMode.workspaceDialog.description', {
                  defaultValue:
                    'Choose one directory and decide how the agent should work in it.',
                })}
            </DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-base py-half">
            <div className="flex flex-col gap-half">
              <label
                htmlFor="workspace-directory-path"
                className="text-sm font-medium text-normal"
              >
                {t('createMode.workspaceDialog.pathLabel', {
                  defaultValue: 'Working directory',
                })}
              </label>
              <div className="flex min-w-0 gap-half">
                <Input
                  id="workspace-directory-path"
                  value={formatPathForDisplay(path)}
                  title={formatPathForDisplay(path)}
                  onChange={(event) => handlePathChange(event.target.value)}
                  onBlur={() => {
                    if (path.trim() && !inspection && !isBusy) {
                      void inspectPath(path);
                    }
                  }}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') {
                      event.preventDefault();
                      if (!isBusy) void inspectPath(path);
                    }
                  }}
                  placeholder={t('createMode.workspaceDialog.pathPlaceholder', {
                    defaultValue: 'C:\\path\\to\\project',
                  })}
                  className="min-w-0 flex-1"
                  aria-invalid={error?.scope === 'path'}
                />
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => void handleBrowse()}
                  disabled={isBusy}
                  className="shrink-0 gap-half"
                >
                  {pendingAction === 'browse' ? (
                    <Loader2 className="size-icon-xs animate-spin" />
                  ) : (
                    <FolderOpen className="size-icon-xs" />
                  )}
                  {t('createMode.workspaceDialog.browse', {
                    defaultValue: 'Browse',
                  })}
                </Button>
              </div>
              {pendingAction === 'inspect' ? (
                <div className="flex items-center gap-half text-xs text-low">
                  <Loader2 className="size-icon-xs animate-spin" />
                  <span>
                    {t('createMode.workspaceDialog.inspecting', {
                      defaultValue: 'Checking directory…',
                    })}
                  </span>
                </div>
              ) : null}
              {error?.scope === 'path' ? (
                <p className="text-xs text-error" role="alert">
                  {error.message}
                </p>
              ) : null}
            </div>

            {inspection && !isProjectDefault ? (
              <fieldset className="flex flex-col gap-half">
                <legend className="mb-half text-sm font-medium text-normal">
                  {t('createMode.workspaceDialog.modeLabel', {
                    defaultValue: 'Working mode',
                  })}
                </legend>
                <div className="flex flex-wrap items-center gap-base">
                  {allowedModeSet.has('direct_folder') ? (
                    <label className="flex min-h-9 cursor-pointer items-center gap-half text-sm text-normal">
                      <input
                        type="radio"
                        name="workspace-working-mode"
                        value="direct_folder"
                        checked={mode === 'direct_folder'}
                        onChange={() => handleModeChange('direct_folder')}
                        className="accent-brand"
                      />
                      {t('createMode.workspaceDialog.directTitle', {
                        defaultValue: 'Direct editing',
                      })}
                    </label>
                  ) : null}
                  {allowedModeSet.has('worktree') ? (
                    <div className="flex items-center gap-half">
                      <label
                        className={cn(
                          'flex min-h-9 items-center gap-half text-sm text-normal',
                          !inspection.is_git_repo || !inspection.repo
                            ? 'cursor-not-allowed opacity-50'
                            : 'cursor-pointer'
                        )}
                      >
                        <input
                          type="radio"
                          name="workspace-working-mode"
                          value="worktree"
                          checked={mode === 'worktree'}
                          disabled={!inspection.is_git_repo || !inspection.repo}
                          onChange={() => handleModeChange('worktree')}
                          className="accent-brand"
                        />
                        {t('createMode.workspaceDialog.worktreeTitle', {
                          defaultValue: 'Isolated Worktree',
                        })}
                      </label>
                      {!inspection.is_git_repo || !inspection.repo ? (
                        <Tooltip
                          content={t(
                            'createMode.workspaceDialog.worktreeUnavailable',
                            {
                              defaultValue: 'Requires a Git repository',
                            }
                          )}
                        >
                          <button
                            type="button"
                            className="flex size-9 items-center justify-center rounded-sm text-low focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-brand"
                            aria-label={t(
                              'createMode.workspaceDialog.worktreeUnavailable',
                              {
                                defaultValue: 'Requires a Git repository',
                              }
                            )}
                          >
                            <Info className="size-icon-xs" aria-hidden="true" />
                          </button>
                        </Tooltip>
                      ) : null}
                    </div>
                  ) : null}
                </div>
                {mode ? (
                  <p className="text-xs text-low">
                    {mode === 'direct_folder'
                      ? t('createMode.workspaceDialog.directDescription', {
                          defaultValue:
                            'The agent will edit files in this directory.',
                        })
                      : t('createMode.workspaceDialog.worktreeDescription', {
                          defaultValue:
                            'Create an isolated checkout; keep the source directory unchanged.',
                        })}
                  </p>
                ) : null}
              </fieldset>
            ) : null}

            {inspection?.repo && mode === 'worktree' ? (
              <div className="flex flex-col gap-half">
                <label className="text-sm font-medium text-normal">
                  {t('createMode.workspaceDialog.branchLabel', {
                    defaultValue: 'Base branch',
                  })}
                </label>
                {pendingAction === 'branches' ? (
                  <div className="flex h-9 items-center gap-half rounded-sm border border-border px-base text-xs text-low">
                    <Loader2 className="size-icon-xs animate-spin" />
                    <span>
                      {t('createMode.workspaceDialog.loadingBranches', {
                        defaultValue: 'Loading branches…',
                      })}
                    </span>
                  </div>
                ) : (
                  <BranchSelector
                    branches={branches}
                    selectedBranch={targetBranch}
                    onBranchSelect={setTargetBranch}
                    placeholder={t(
                      'createMode.workspaceDialog.branchPlaceholder',
                      { defaultValue: 'Choose a branch' }
                    )}
                  />
                )}
                {error?.scope === 'branch' ? (
                  <p className="text-xs text-error" role="alert">
                    {error.message}
                  </p>
                ) : null}
              </div>
            ) : null}
          </div>

          <DialogFooter>
            <Button type="button" variant="ghost" onClick={handleCancel}>
              {t('buttons.cancel')}
            </Button>
            <Button
              type="button"
              onClick={handleConfirm}
              disabled={!canConfirm}
              className="gap-half"
            >
              {mode === 'worktree' ? (
                <GitBranchIcon className="size-icon-xs" />
              ) : (
                <Folder className="size-icon-xs" />
              )}
              {confirmLabel}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    );
  }
);

export const WorkspaceTargetDialog = defineModal<
  WorkspaceTargetDialogProps,
  WorkspaceTargetDialogResult
>(WorkspaceTargetDialogImpl);
