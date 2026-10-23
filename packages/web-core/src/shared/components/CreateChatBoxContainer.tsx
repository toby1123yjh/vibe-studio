import { useMemo, useCallback, useState, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useDropzone } from 'react-dropzone';
import { useSessionProjectTarget } from '@/shared/hooks/useSessionProjectTarget';
import { useCreateMode } from '@/features/create-mode/model/useCreateMode';
import { AgentIcon, getAgentName } from '@/shared/components/AgentIcon';
import { getAgentBrandAttributes } from '@/shared/lib/agentProviders';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import WYSIWYGEditor from '@/shared/components/WYSIWYGEditor';
import { useCreateWorkspace } from '@/shared/hooks/useCreateWorkspace';
import { useCreateAttachments } from '@/shared/hooks/useCreateAttachments';
import { useExecutorConfig } from '@/shared/hooks/useExecutorConfig';
import { saveProjectWorkspaceDefault } from '@/shared/hooks/useProjectRepoDefaults';
import { useCurrentAppDestination } from '@/shared/hooks/useCurrentAppDestination';
import { getSortedExecutorVariantKeys } from '@/shared/lib/executor';
import { getDestinationHostId } from '@/shared/lib/routes/appNavigation';
import { buildAgentPrompt } from '@/shared/lib/promptMessage';
import { resolveWorkspaceWorkingDirectory } from '@/shared/lib/workspaceContext';
import { splitMessageToTitleDescription } from '@/shared/lib/string';
import type {
  BaseCodingAgent,
  ResumableAgentSession,
  SelectedSkill,
} from 'shared/types';
import { CreateChatBox } from '@vibe/ui/components/CreateChatBox';
import type { LinkedTask } from '@/shared/types/createMode';
import { useSettingsNavigation } from '@/shared/hooks/useSettingsNavigation';
import { WorkspaceTargetDialog } from '@/shared/dialogs/shared/WorkspaceTargetDialog';
import { ModelSelectorContainer } from '@/shared/components/ModelSelectorContainer';
import { useModelAttachmentGuard } from '@/shared/hooks/useModelAttachmentGuard';
import {
  AgentSessionResumeChip,
  AgentSessionResumePicker,
} from '@/shared/components/AgentSessionResumePicker';

interface CreateChatBoxContainerProps {
  onWorkspaceCreated: (workspaceId: string) => void;
  requiredLinkedTask?: LinkedTask;
}

export function CreateChatBoxContainer({
  onWorkspaceCreated,
  requiredLinkedTask,
}: CreateChatBoxContainerProps) {
  const { t } = useTranslation('common');
  const { openAgentCenter } = useSettingsNavigation();
  const { profiles, config } = useUserSystem();
  const {
    initialProjectId,
    message,
    setMessage,
    clearDraft,
    hasInitialValue,
    linkedTask: draftLinkedTask,
    clearLinkedTask,
    preferredExecutorConfig,
    executorConfig: draftConfig,
    setExecutorConfig: setDraftConfig,
    attachments: draftAttachments,
    setAttachments: setDraftAttachments,
  } = useCreateMode();
  const linkedTask = requiredLinkedTask ?? draftLinkedTask;

  const { createWorkspace } = useCreateWorkspace();
  const destination = useCurrentAppDestination();
  const hostId = useMemo(
    () => getDestinationHostId(destination),
    [destination]
  );
  const [targetError, setTargetError] = useState<string | null>(null);
  const [isConfiguring, setIsConfiguring] = useState(false);
  const workspaceDialogOpenRef = useRef(false);
  const submitPendingRef = useRef(false);
  const destinationProjectId =
    destination && 'projectId' in destination
      ? destination.projectId
      : undefined;
  const project = useSessionProjectTarget(
    hostId,
    destinationProjectId ?? initialProjectId,
    linkedTask?.remoteProjectId
  );
  const scopeIdentity = `${hostId ?? 'local'}:${project.projectId}:${linkedTask?.taskId ?? 'standalone'}`;
  const scopeRef = useRef({
    identity: scopeIdentity,
    generation: 0,
    mounted: true,
  });
  if (scopeRef.current.identity !== scopeIdentity) {
    scopeRef.current = {
      identity: scopeIdentity,
      generation: scopeRef.current.generation + 1,
      mounted: true,
    };
  }
  useEffect(() => {
    scopeRef.current.mounted = true;
    return () => {
      scopeRef.current.mounted = false;
      scopeRef.current.generation += 1;
    };
  }, []);
  useEffect(() => {
    setTargetError(null);
    setStagedResumeSession(null);
  }, [scopeIdentity]);
  const [selectedSkills, setSelectedSkills] = useState<SelectedSkill[]>([]);
  const [stagedResumeSession, setStagedResumeSession] =
    useState<ResumableAgentSession | null>(null);
  const target = project.target.data;
  const workspaceMode = target?.mode ?? 'managed_directory';
  const selectedRepo = target?.mode === 'worktree' ? target.repo : null;
  const selectedTargetBranch =
    target?.mode === 'worktree' ? target.branch : null;
  const directFolderPath = target?.mode === 'direct_folder' ? target.path : '';

  // Attachment handling - insert markdown and track attachment IDs
  const handleInsertMarkdown = useCallback(
    (markdown: string) => {
      const newMessage = message.trim()
        ? `${message}\n\n${markdown}`
        : markdown;
      setMessage(newMessage);
    },
    [message, setMessage]
  );

  const { uploadFiles, getAttachmentIds, clearAttachments, localAttachments } =
    useCreateAttachments(
      handleInsertMarkdown,
      draftAttachments,
      setDraftAttachments
    );

  const scratchConfig = useMemo(() => {
    if (!hasInitialValue) return undefined; // still loading
    return draftConfig ?? null;
  }, [hasInitialValue, draftConfig]);

  const {
    executorConfig,
    effectiveExecutor,
    selectedVariant,
    executorOptions,
    variantOptions,
    presetOptions,
    setOverrides: setExecutorOverrides,
  } = useExecutorConfig({
    profiles,
    lastUsedConfig: preferredExecutorConfig,
    scratchConfig,
    configExecutorProfile: config?.executor_profile,
    hiddenAgents: config?.hidden_agents,
    onPersist: (cfg) => setDraftConfig(cfg),
  });
  const attachmentGuard = useModelAttachmentGuard(
    JSON.stringify([
      scopeIdentity,
      effectiveExecutor,
      selectedVariant,
      executorConfig?.model_id,
    ]),
    uploadFiles
  );
  const { getRootProps, getInputProps, isDragActive } = useDropzone({
    onDrop: attachmentGuard.uploadFiles,
    disabled: createWorkspace.isPending,
    noClick: true,
    noKeyboard: true,
  });

  useEffect(() => {
    setStagedResumeSession(null);
  }, [effectiveExecutor]);

  const repoId = selectedRepo?.id;
  const canSubmit =
    hasInitialValue &&
    project.ready &&
    !isConfiguring &&
    !createWorkspace.isPending &&
    message.trim().length > 0 &&
    effectiveExecutor !== null;

  const configureProjectWorkspace = async () => {
    if (workspaceDialogOpenRef.current || !project.enabled || !project.exists)
      return;
    workspaceDialogOpenRef.current = true;
    setIsConfiguring(true);
    setTargetError(null);
    const generation = scopeRef.current.generation;
    try {
      const result = await WorkspaceTargetDialog.show({ hostId });
      if (
        result.kind !== 'confirmed' ||
        !scopeRef.current.mounted ||
        scopeRef.current.generation !== generation
      )
        return;
      await saveProjectWorkspaceDefault(
        project.projectId,
        result.selection.mode === 'worktree'
          ? {
              kind: 'git',
              repo: {
                repo_id: result.selection.repo.id,
                target_branch: result.selection.targetBranch,
              },
            }
          : { kind: 'direct_folder', path: result.selection.path },
        hostId
      );
      if (
        !scopeRef.current.mounted ||
        scopeRef.current.generation !== generation
      )
        return;
      await project.target.refetch();
    } catch (error) {
      if (
        scopeRef.current.mounted &&
        scopeRef.current.generation === generation
      )
        setTargetError(error instanceof Error ? error.message : String(error));
    } finally {
      workspaceDialogOpenRef.current = false;
      setIsConfiguring(false);
    }
  };

  const handlePresetSelect = (presetId: string | null) => {
    if (!effectiveExecutor) return;
    setDraftConfig({
      ...draftConfig,
      executor: effectiveExecutor,
      variant: presetId,
    });
  };

  const handleCustomise = () => {
    openAgentCenter();
  };

  const resumeScopePath = resolveWorkspaceWorkingDirectory({
    containerRef:
      workspaceMode === 'managed_directory'
        ? undefined
        : workspaceMode === 'direct_folder'
          ? directFolderPath
          : selectedRepo?.path,
  });

  useEffect(() => {
    setStagedResumeSession(null);
  }, [resumeScopePath, workspaceMode]);

  const resumePickerNode = effectiveExecutor ? (
    <AgentSessionResumePicker
      scopePath={resumeScopePath}
      executor={effectiveExecutor}
      selectedSessionId={stagedResumeSession?.agent_session_id}
      disabled={createWorkspace.isPending || !resumeScopePath}
      onSelect={setStagedResumeSession}
    />
  ) : undefined;

  const modelSelectorNode =
    effectiveExecutor || stagedResumeSession ? (
      <>
        {effectiveExecutor && (
          <ModelSelectorContainer
            agent={effectiveExecutor}
            workspaceId={undefined}
            onAdvancedSettings={handleCustomise}
            presets={variantOptions}
            selectedPreset={selectedVariant}
            onPresetSelect={handlePresetSelect}
            onOverrideChange={setExecutorOverrides}
            executorConfig={executorConfig}
            presetOptions={presetOptions}
            onInputModalitiesChange={attachmentGuard.onInputModalitiesChange}
          />
        )}
        {attachmentGuard.notice && (
          <span role="status" className="text-sm text-low">
            {attachmentGuard.notice}
          </span>
        )}
        {stagedResumeSession && (
          <AgentSessionResumeChip
            session={stagedResumeSession}
            onClear={() => setStagedResumeSession(null)}
          />
        )}
      </>
    ) : undefined;

  // Handle executor change - use saved variant if switching to default executor
  const handleExecutorChange = useCallback(
    (executor: BaseCodingAgent) => {
      setStagedResumeSession(null);
      const executorProfile = profiles?.[executor];
      if (!executorProfile) {
        setDraftConfig({ executor, variant: null });
        return;
      }

      const variants = getSortedExecutorVariantKeys(executorProfile);
      let targetVariant: string | null = null;

      // If switching to user's default executor, use their saved variant
      if (
        config?.executor_profile?.executor === executor &&
        config?.executor_profile?.variant
      ) {
        const savedVariant = config.executor_profile.variant;
        if (variants.includes(savedVariant)) {
          targetVariant = savedVariant;
        }
      }

      // Fallback to DEFAULT or first available
      if (!targetVariant) {
        targetVariant = variants.includes('DEFAULT')
          ? 'DEFAULT'
          : (variants[0] ?? null);
      }

      setDraftConfig({ executor, variant: targetVariant });
    },
    [profiles, setDraftConfig, config?.executor_profile]
  );

  // Handle submit
  const handleSubmit = useCallback(async () => {
    if (!canSubmit || !executorConfig || submitPendingRef.current) return;
    submitPendingRef.current = true;
    const generation = scopeRef.current.generation;
    try {
      const { title } = splitMessageToTitleDescription(message);
      const { prompt, isSlashCommand } = buildAgentPrompt(message, []);
      const data = {
        project_id: project.projectId,
        mode: workspaceMode,
        executor_config: executorConfig,
        name: title,
        prompt,
        repos:
          workspaceMode === 'worktree' && selectedRepo && selectedTargetBranch
            ? [
                {
                  repo_id: selectedRepo.id,
                  target_branch: selectedTargetBranch,
                },
              ]
            : [],
        directory_path:
          workspaceMode === 'direct_folder'
            ? directFolderPath.trim()
            : undefined,
        linked_task: linkedTask
          ? {
              remote_project_id: linkedTask.remoteProjectId,
              task_id: linkedTask.taskId,
            }
          : null,
        selected_skills:
          !isSlashCommand && selectedSkills.length > 0
            ? selectedSkills
            : undefined,
        resume_session_id: stagedResumeSession?.agent_session_id,
        resume_scope_path: resumeScopePath,
        attachment_ids: getAttachmentIds(),
      };
      const linkToTask = linkedTask
        ? {
            remoteProjectId: linkedTask.remoteProjectId,
            taskId: linkedTask.taskId,
          }
        : undefined;

      const result = await createWorkspace.mutateAsync({
        data,
        linkToTask,
      });
      if (
        !scopeRef.current.mounted ||
        scopeRef.current.generation !== generation
      )
        return;

      if (result.workspace) {
        onWorkspaceCreated(result.workspace.id);
      }

      clearAttachments();
      setSelectedSkills([]);
      setStagedResumeSession(null);
      await clearDraft();
    } finally {
      submitPendingRef.current = false;
    }
  }, [
    canSubmit,
    project.projectId,
    executorConfig,
    message,
    selectedSkills,
    stagedResumeSession?.agent_session_id,
    workspaceMode,
    directFolderPath,
    selectedRepo,
    selectedTargetBranch,
    resumeScopePath,
    createWorkspace,
    onWorkspaceCreated,
    getAttachmentIds,
    clearAttachments,
    clearDraft,
    linkedTask,
  ]);

  // Determine error to display
  const errorDetails =
    targetError ??
    (createWorkspace.error
      ? createWorkspace.error instanceof Error
        ? createWorkspace.error.message
        : t('sessionProject.failed')
      : null);
  const displayError = targetError
    ? t('sessionProject.configureFailed')
    : createWorkspace.error
      ? t('sessionProject.createFailed')
      : null;

  const projectSelector = (
    <label className="inline-flex min-w-0 max-w-[240px] items-center gap-half text-sm text-low">
      <span className="shrink-0">{t('sessionProject.label')}</span>
      <select
        aria-label={t('sessionProject.label')}
        value={project.projectId}
        disabled={
          createWorkspace.isPending ||
          isConfiguring ||
          !!linkedTask ||
          !project.enabled ||
          project.projects.isPending
        }
        onChange={(event) => project.selectProject(event.target.value)}
        className="min-h-8 min-w-0 max-w-full appearance-none rounded-sm bg-transparent px-half font-medium text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand disabled:cursor-default disabled:opacity-70"
      >
        {!project.exists && (
          <option value={project.projectId}>{t('sessionProject.label')}</option>
        )}
        {project.projects.data?.map((item) => (
          <option key={item.id} value={item.id}>
            {item.name}
          </option>
        ))}
      </select>
    </label>
  );

  // Wait for initial value to be applied before rendering
  // This ensures the editor mounts with content ready, so autoFocus works correctly
  if (!hasInitialValue) {
    return null;
  }

  return (
    <div className="relative flex flex-1 flex-col bg-primary h-full">
      <div className="flex flex-1 items-center justify-center px-base">
        <div className="flex w-chat max-w-full flex-col gap-base">
          {!project.enabled && (
            <p role="alert">{t('defaultProject.offline')}</p>
          )}
          {project.enabled &&
            (project.projects.isError ||
              project.target.isError ||
              (!project.projects.isPending && !project.exists)) && (
              <div role="alert">
                <p>{t('sessionProject.failed')}</p>
                <button
                  type="button"
                  className="underline"
                  onClick={() => {
                    void project.projects.refetch();
                    if (project.exists) void project.target.refetch();
                  }}
                >
                  {t('buttons.retry')}
                </button>
              </div>
            )}
          {project.enabled &&
            (project.projects.isPending ||
              (project.exists && project.target.isFetching)) && (
              <p role="status">{t('states.loading')}</p>
            )}
          {project.exists &&
            project.target.isSuccess &&
            project.target.data === null && (
              <div className="text-sm text-low">
                <p>{t('sessionProject.missingWorkspace')}</p>
                <button
                  type="button"
                  disabled={isConfiguring}
                  className="min-h-9 underline"
                  onClick={() => void configureProjectWorkspace()}
                >
                  {t('sessionProject.configure')}
                </button>
              </div>
            )}
          <>
            <h2 className="mb-double text-center text-4xl font-medium tracking-tight text-high">
              {t('createMode.headings.chatStep')}
            </h2>

            <div className="flex justify-center @container">
              <CreateChatBox
                editor={{
                  value: message,
                  onChange: setMessage,
                }}
                renderEditor={({
                  value,
                  onChange,
                  onCmdEnter,
                  disabled,
                  repoIds,
                  repoId,
                  executor,
                  onPasteFiles,
                  localAttachments,
                }) => (
                  <WYSIWYGEditor
                    placeholder={t('sessionProject.messagePlaceholder')}
                    value={value}
                    onChange={onChange}
                    onCmdEnter={onCmdEnter}
                    disabled={disabled}
                    className="min-h-[3lh] min-w-0 [&_code.block]:whitespace-pre-wrap [&_code.block]:[overflow-wrap:anywhere]"
                    repoIds={repoIds}
                    repoId={repoId}
                    executor={executor}
                    selectedSkills={selectedSkills}
                    onSelectedSkillsChange={setSelectedSkills}
                    autoFocus
                    onPasteFiles={onPasteFiles}
                    localAttachments={localAttachments}
                    sendShortcut={config?.send_message_shortcut}
                  />
                )}
                agentIcon={
                  <AgentIcon
                    agent={effectiveExecutor}
                    className="vk-agent-brand-icon size-icon-xl"
                  />
                }
                agentBrand={
                  getAgentBrandAttributes(effectiveExecutor)['data-agent-brand']
                }
                onSend={handleSubmit}
                isSending={createWorkspace.isPending}
                sendDisabled={!canSubmit}
                executor={{
                  selected: effectiveExecutor,
                  options: executorOptions,
                  onChange: handleExecutorChange,
                  afterSelector: resumePickerNode,
                }}
                formatExecutorLabel={getAgentName}
                error={displayError}
                repoIds={
                  workspaceMode === 'worktree' && selectedRepo
                    ? [selectedRepo.id]
                    : []
                }
                repoId={workspaceMode === 'worktree' ? repoId : undefined}
                modelSelector={modelSelectorNode}
                onPasteFiles={attachmentGuard.uploadFiles}
                localAttachments={localAttachments}
                dropzone={{ getRootProps, getInputProps, isDragActive }}
                projectSelector={projectSelector}
                linkedTask={
                  linkedTask?.simpleId
                    ? {
                        simpleId: linkedTask.simpleId,
                        title: linkedTask.title ?? '',
                        onRemove: requiredLinkedTask
                          ? undefined
                          : clearLinkedTask,
                      }
                    : null
                }
              />
            </div>
            {errorDetails && (
              <details className="text-sm text-low">
                <summary className="cursor-pointer">
                  {t('errors.details')}
                </summary>
                <pre className="mt-half whitespace-pre-wrap break-words">
                  {errorDetails}
                </pre>
              </details>
            )}
          </>
        </div>
      </div>
    </div>
  );
}
