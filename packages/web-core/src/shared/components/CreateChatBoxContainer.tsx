import { useMemo, useCallback, useState, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useDropzone } from 'react-dropzone';
import { useSessionProjectTarget } from '@/shared/hooks/useSessionProjectTarget';
import { useCreateMode } from '@/features/create-mode/model/useCreateMode';
import {
  acceptConversationSend,
  beginConversationSend,
  bindConversationSend,
  consumeFailedConversationDraft,
  failConversationSend,
  markConversationDraftEdited,
  usePendingConversationSend,
} from '@/features/workspace-chat/model/pendingConversationSends';
import { AgentIcon, getAgentName } from '@/shared/components/AgentIcon';
import { getAgentBrandAttributes } from '@/shared/lib/agentProviders';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import WYSIWYGEditor, {
  type WYSIWYGEditorRef,
} from '@/shared/components/WYSIWYGEditor';
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
import { ChatUserMessage } from '@vibe/ui/components/ChatUserMessage';
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
    draftId,
    initialProjectId,
    message,
    setMessage,
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
  const scopeIdentity = `${hostId ?? 'local'}:${project.projectId}:${linkedTask?.taskId ?? 'standalone'}:${draftId ?? 'default'}`;
  const composerEditorRef = useRef<WYSIWYGEditorRef>(null);
  const creationScope = useMemo(
    () => ({
      hostId,
      workspaceId: undefined,
      sessionId: undefined,
      draftId: `create:${scopeIdentity}`,
    }),
    [hostId, scopeIdentity]
  );
  const pendingCreation = usePendingConversationSend(creationScope);
  const isCreating =
    createWorkspace.isPending ||
    pendingCreation.sends.some((send) => send.phase === 'sending');
  const [failedCreationText, setFailedCreationText] = useState<string | null>(
    null
  );
  const draftIdentityRef = useRef({
    scope: scopeIdentity,
    revision: 0,
    text: message,
  });
  if (draftIdentityRef.current.scope !== scopeIdentity) {
    draftIdentityRef.current = {
      scope: scopeIdentity,
      revision: 0,
      text: message,
    };
  } else if (draftIdentityRef.current.text !== message) {
    draftIdentityRef.current.text = message;
    draftIdentityRef.current.revision += 1;
  }
  const clearedCreationDraftRef = useRef<{
    id: string;
    revision: number;
  } | null>(null);
  const handleMessageChange = useCallback(
    (value: string) => {
      if (draftIdentityRef.current.text !== value) {
        draftIdentityRef.current.revision += 1;
        draftIdentityRef.current.text = value;
        markConversationDraftEdited(creationScope);
      }
      setMessage(value);
    },
    [creationScope, setMessage]
  );
  const scopeRef = useRef({
    identity: scopeIdentity,
    generation: 0,
    mounted: true,
  });
  const releaseCreationFeedbackRef = useRef<(() => void) | undefined>();
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
  useEffect(
    () => () => {
      releaseCreationFeedbackRef.current?.();
      releaseCreationFeedbackRef.current = undefined;
    },
    [scopeIdentity]
  );
  useEffect(() => {
    setTargetError(null);
    setStagedResumeSession(null);
    setFailedCreationText(null);
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
      const currentMessage =
        composerEditorRef.current?.getMarkdown() ??
        draftIdentityRef.current.text;
      const newMessage = currentMessage.trim()
        ? `${currentMessage}\n\n${markdown}`
        : markdown;
      handleMessageChange(newMessage);
    },
    [handleMessageChange]
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
  const controlsIdentity = JSON.stringify([
    executorConfig,
    draftAttachments,
    selectedSkills,
    stagedResumeSession,
  ]);
  const controlsIdentityRef = useRef(controlsIdentity);
  if (controlsIdentityRef.current !== controlsIdentity) {
    controlsIdentityRef.current = controlsIdentity;
    draftIdentityRef.current.revision += 1;
  }

  useEffect(() => {
    const failed = pendingCreation.failedDraft;
    if (!failed || !hasInitialValue) return;
    const clear = clearedCreationDraftRef.current;
    const expectedRevision = clear?.id === failed.id ? clear.revision : 0;
    setFailedCreationText(failed.text);
    if (
      draftIdentityRef.current.revision === expectedRevision &&
      draftIdentityRef.current.text === ''
    ) {
      draftIdentityRef.current.revision += 1;
      draftIdentityRef.current.text = failed.text;
      setMessage(failed.text);
    }
    consumeFailedConversationDraft(creationScope);
  }, [pendingCreation.failedDraft, hasInitialValue, creationScope, setMessage]);
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
    disabled: !hasInitialValue,
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
    !isCreating &&
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
      disabled={isCreating || !resumeScopePath}
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
  const handleSubmit = useCallback(
    async (submittedMessage: string) => {
      if (
        !canSubmit ||
        !submittedMessage.trim() ||
        !executorConfig ||
        submitPendingRef.current
      )
        return;
      const firstMessage = beginConversationSend({
        ...creationScope,
        text: submittedMessage,
        executorConfig,
      });
      if (!firstMessage) return;
      const creationHandle = { ...firstMessage };
      submitPendingRef.current = true;
      const generation = scopeRef.current.generation;
      const submittedAttachmentIds = getAttachmentIds();
      setFailedCreationText(null);
      draftIdentityRef.current.text = '';
      const clearRevision = ++draftIdentityRef.current.revision;
      clearedCreationDraftRef.current = {
        id: firstMessage.id,
        revision: clearRevision,
      };
      setMessage('');
      composerEditorRef.current?.clear();
      try {
        const { title } = splitMessageToTitleDescription(submittedMessage);
        const { prompt, isSlashCommand } = buildAgentPrompt(
          submittedMessage,
          []
        );
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
          attachment_ids: submittedAttachmentIds,
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
        if (!result.workspace)
          throw new Error(t('sessionProject.createFailed'));

        // Transfer before selecting the workspace. Its first history fetch may
        // lag behind the create response, so the shared window keeps this echo.
        if (result.workspace) {
          const retainSource =
            scopeRef.current.mounted &&
            scopeRef.current.generation === generation;
          const releaseSource = bindConversationSend(
            firstMessage,
            {
              hostId,
              workspaceId: result.workspace.id,
              sessionId: result.agentRun.state.session_id,
            },
            { retainSource }
          );
          if (retainSource) releaseCreationFeedbackRef.current = releaseSource;
          acceptConversationSend(firstMessage, result.agentRun.agent_run_id);
        }
        if (
          !scopeRef.current.mounted ||
          scopeRef.current.generation !== generation
        )
          return;

        clearAttachments(submittedAttachmentIds ?? []);
        if (
          draftIdentityRef.current.revision === clearRevision &&
          draftIdentityRef.current.text === ''
        ) {
          setSelectedSkills([]);
          setStagedResumeSession(null);
          clearLinkedTask();
        }
        if (result.workspace) onWorkspaceCreated(result.workspace.id);
      } catch (error) {
        const errorMessage =
          error instanceof Error ? error.message : String(error);
        // Navigation can fail after binding. Restore the original draft scope
        // as well, rather than leaving its outgoing feedback stuck as sending.
        if (firstMessage.scopeKey !== creationHandle.scopeKey)
          failConversationSend(creationHandle, errorMessage);
        releaseCreationFeedbackRef.current?.();
        releaseCreationFeedbackRef.current = undefined;
        failConversationSend(firstMessage, errorMessage);
        if (
          scopeRef.current.mounted &&
          scopeRef.current.generation === generation
        ) {
          setFailedCreationText(submittedMessage);
        }
      } finally {
        submitPendingRef.current = false;
      }
    },
    [
      canSubmit,
      project.projectId,
      executorConfig,
      hostId,
      creationScope,
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
      setMessage,
      clearLinkedTask,
      linkedTask,
      t,
    ]
  );

  // Determine error to display
  const errorDetails =
    targetError ??
    (createWorkspace.error
      ? createWorkspace.error instanceof Error
        ? createWorkspace.error.message
        : t('sessionProject.failed')
      : pendingCreation.error);
  const displayError = targetError
    ? t('sessionProject.configureFailed')
    : createWorkspace.error || pendingCreation.error
      ? t('sessionProject.createFailed')
      : null;

  const projectSelector = (
    <label className="inline-flex min-w-0 max-w-[240px] items-center gap-half text-sm text-low">
      <span className="shrink-0">{t('sessionProject.label')}</span>
      <select
        aria-label={t('sessionProject.label')}
        value={project.projectId}
        disabled={
          isCreating ||
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
            <h2
              className="mb-double text-center text-4xl font-medium tracking-tight text-high"
              hidden={pendingCreation.sends.length > 0 || !!failedCreationText}
            >
              {t('createMode.headings.chatStep')}
            </h2>

            {(pendingCreation.sends.length > 0 || failedCreationText) && (
              <div
                className="flex min-w-0 flex-col gap-base px-plusfifty py-double"
                data-creation-conversation
              >
                {pendingCreation.sends.map((send) => (
                  <ChatUserMessage
                    key={send.id}
                    content={send.text}
                    submissionStatus={send.phase}
                    renderMarkdown={({ content }) => (
                      <WYSIWYGEditor
                        value={content}
                        disabled
                        hideActions
                        localAttachments={localAttachments}
                      />
                    )}
                  />
                ))}
                {failedCreationText && pendingCreation.sends.length === 0 && (
                  <ChatUserMessage
                    content={failedCreationText}
                    renderMarkdown={({ content }) => (
                      <WYSIWYGEditor
                        value={content}
                        disabled
                        hideActions
                        localAttachments={localAttachments}
                      />
                    )}
                  />
                )}
              </div>
            )}

            <div className="flex justify-center @container">
              <CreateChatBox
                editor={{
                  value: message,
                  onChange: handleMessageChange,
                  getValue: () =>
                    composerEditorRef.current?.getMarkdown() ??
                    draftIdentityRef.current.text,
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
                    ref={composerEditorRef}
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
                isSending={isCreating}
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
