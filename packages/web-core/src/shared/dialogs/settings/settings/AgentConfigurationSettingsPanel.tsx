import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  ArrowClockwiseIcon,
  CheckCircleIcon,
  CodeIcon,
  CopyIcon,
  EyeIcon,
  EyeSlashIcon,
  FloppyDiskIcon,
  PencilSimpleIcon,
  PlusIcon,
  SpinnerIcon,
  TrashIcon,
  WarningCircleIcon,
} from '@phosphor-icons/react';
import { PrimaryButton } from '@vibe/ui/components/PrimaryButton';
import { Button } from '@vibe/ui/components/Button';
import { ConfirmDialog } from '@vibe/ui/components/ConfirmDialog';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/KeyboardDialog';
import { Switch } from '@vibe/ui/components/Switch';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import {
  AgentSettingsProvider,
  BaseCodingAgent,
  SettingScope,
} from 'shared/types';
import type {
  ConfigProfileView,
  CopyProfilePreviewRequest,
  JsonValue,
  ProfileApplyPreviewRequest,
  ProfileCopyPreview,
  SettingDescriptor,
  SettingsDiff,
  SettingsPatch,
  SettingsSnapshot,
} from 'shared/types';
import {
  agentSettingSourceKind,
  buildAgentSettingsPatch,
  buildAgentSettingsSections,
  buildNativeConfigFileModels,
  createAgentSettingsDraft,
  formatProfileEnvironment,
  hasChangedFiles,
  hasDraftErrors,
  isAgentSettingsContextCurrent,
  isAgentSettingsRequestCurrent,
  isAgentSettingsDraftDirty,
  isRevealResponseCurrent,
  nativeFileRevision,
  parseSettingInput,
  parseProfileCustomArgs,
  parseProfileEnvironment,
  profileEnvironmentFromSnapshot,
  PROVIDER_BY_EXECUTOR,
  PROVIDER_LABELS,
  settingSourceForScope,
  settingKeyId,
  type AgentSettingsDraft,
  type AgentSettingsRequestContext,
} from '@/shared/lib/agentSettingsModel';
import {
  formatAgentSettingOperationError,
  type MachineClient,
} from '@/shared/lib/machineClient';
import { cn } from '@/shared/lib/utils';
import { agentProviderForSettings } from '@/shared/lib/agentProviders';
import { AgentToolsSettingsSection } from './AgentToolsSettingsSection';
import {
  SettingsCard,
  SettingsField,
  SettingsInput,
  SettingsSaveBar,
  SettingsTextarea,
} from './SettingsComponents';
import { useSettingsDirty } from './SettingsDirtyContext';
import { useSettingsMachineClient } from './SettingsHostContext';

type PendingAction =
  | {
      kind: 'settings';
      patch: SettingsPatch;
      context: AgentSettingsRequestContext;
    }
  | {
      kind: 'profile';
      preview: ProfileApplyPreviewRequest;
      context: AgentSettingsRequestContext;
    };

type RevealedSetting = {
  id: string;
  value: string;
};

type ConfigurationNotice =
  | 'noChanges'
  | 'applied'
  | 'profileSaved'
  | 'profileUpdated'
  | 'profileDuplicated'
  | 'profileDeleted'
  | 'copyUpdated'
  | 'copySaved';

type ConfigurationError =
  | string
  | { key: 'invalidSettings' | 'invalidEnvironment' };

type ProfileEditorDraft = {
  id: string;
  name: string;
  environmentRaw: string;
  customArgsRaw: string;
  initialName: string;
  initialEnvironmentRaw: string;
  initialCustomArgsRaw: string;
};

type ScopedCopyPreview = {
  preview: ProfileCopyPreview;
  request: CopyProfilePreviewRequest;
  context: AgentSettingsRequestContext;
};

type AgentSettingsOperation = {
  sequence: number;
  context: AgentSettingsRequestContext;
  client: MachineClient;
};

type ProfileActionDraft =
  | {
      kind: 'create';
      name: string;
      initialName: string;
    }
  | {
      kind: 'duplicate';
      sourceId: string;
      sourceName: string;
      name: string;
      initialName: string;
    }
  | {
      kind: 'copy';
      sourceId: string;
      sourceName: string;
      name: string;
      initialName: string;
      targetProvider: AgentSettingsProvider;
      initialTargetProvider: AgentSettingsProvider;
      targetProfileId: string | null;
      initialTargetProfileId: string | null;
    };

function newProfileId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID();
  }
  return `profile-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function executorForProvider(provider: AgentSettingsProvider): BaseCodingAgent {
  return agentProviderForSettings(provider).executor;
}

function displayJson(value: JsonValue | undefined, unsetLabel: string): string {
  if (value === undefined) return unsetLabel;
  return JSON.stringify(value, null, 2);
}

function sourceLabelKind(
  snapshot: SettingsSnapshot,
  descriptor: SettingDescriptor,
  entry: AgentSettingsDraft[string] | undefined
): ReturnType<typeof agentSettingSourceKind> | 'modified' | 'inherit' {
  if (entry?.action === 'replace') return 'modified';
  if (entry?.action === 'clear') return 'inherit';
  const setting = snapshot.effective_settings.find(
    (candidate) => settingKeyId(candidate) === settingKeyId(descriptor)
  );
  return agentSettingSourceKind(setting?.effective_source);
}

function parseDraftChange(
  descriptor: SettingDescriptor,
  raw: string,
  value?: JsonValue
): { value?: JsonValue; error?: string } {
  if (descriptor.control === 'select') {
    return parseSettingInput(descriptor, raw);
  }
  if (descriptor.control === 'toggle') return { value };
  return parseSettingInput(descriptor, raw);
}

export function AgentConfigurationSettingsPanel({
  executor,
  variant,
  includeTools = true,
}: {
  executor: BaseCodingAgent;
  variant: string | null;
  includeTools?: boolean;
}) {
  const { t } = useTranslation('common');
  const { t: tc, i18n } = useTranslation('agent-config');
  const sourceLabel = (kind: ReturnType<typeof sourceLabelKind>): string => {
    switch (kind) {
      case 'modified':
        return tc('modified');
      case 'inherit':
        return t('agentCenter.inheritedOrUnset');
      case 'native_user':
        return t('agentCenter.configurationSources.nativeUser');
      case 'native_project':
        return t('agentCenter.configurationSources.nativeProject');
      case 'adapter_managed':
        return t('agentCenter.adapterManaged');
      default:
        return t('agentCenter.inheritedOrUnset');
    }
  };
  const machineClient = useSettingsMachineClient();
  const { setDirty: setContextDirty } = useSettingsDirty();
  const provider =
    PROVIDER_BY_EXECUTOR[executor] ?? AgentSettingsProvider.codex;
  const [loadedSnapshot, setLoadedSnapshot] = useState<SettingsSnapshot | null>(
    null
  );
  const [snapshotContext, setSnapshotContext] =
    useState<AgentSettingsRequestContext | null>(null);
  const [draft, setDraft] = useState<AgentSettingsDraft>({});
  const [loadedProfiles, setLoadedProfiles] = useState<ConfigProfileView[]>([]);
  const [profilesContext, setProfilesContext] =
    useState<AgentSettingsRequestContext | null>(null);
  const [activeSection, setActiveSection] = useState<string>('overview');
  const [scope, setScope] = useState<SettingScope>(SettingScope.user);
  const [projectPath, setProjectPath] = useState('');
  const [projectPathInput, setProjectPathInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [profilesLoading, setProfilesLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ConfigurationError | null>(null);
  const [notice, setNotice] = useState<ConfigurationNotice | null>(null);
  const [diff, setDiff] = useState<SettingsDiff | null>(null);
  const [pending, setPending] = useState<PendingAction | null>(null);
  const [scopedCopyPreview, setScopedCopyPreview] =
    useState<ScopedCopyPreview | null>(null);
  const [revealedSetting, setRevealedSetting] =
    useState<RevealedSetting | null>(null);
  const [visibleReplacementIds, setVisibleReplacementIds] = useState<
    Record<string, boolean>
  >({});
  const [revealingSettingId, setRevealingSettingId] = useState<string | null>(
    null
  );
  const [revealErrors, setRevealErrors] = useState<Record<string, string>>({});
  const [profileEnvironmentRaw, setProfileEnvironmentRaw] = useState('{}');
  const [profileCustomArgsRaw, setProfileCustomArgsRaw] = useState('');
  const [profileFieldsDirty, setProfileFieldsDirty] = useState(false);
  const [profileEditor, setProfileEditor] = useState<ProfileEditorDraft | null>(
    null
  );
  const [profileAction, setProfileAction] = useState<ProfileActionDraft | null>(
    null
  );
  const settingsRequestSequence = useRef(0);
  const profileRequestSequence = useRef(0);
  const operationRequestSequence = useRef(0);
  const revealRequestSequence = useRef(0);
  const activeMachineClient = useRef(machineClient);
  const activeRequestContext = useRef<AgentSettingsRequestContext>({
    client: machineClient,
    provider,
    projectPath,
    scope,
  });
  activeMachineClient.current = machineClient;
  activeRequestContext.current = {
    client: machineClient,
    provider,
    projectPath,
    scope,
  };
  const snapshot =
    snapshotContext &&
    isAgentSettingsContextCurrent(snapshotContext, activeRequestContext.current)
      ? loadedSnapshot
      : null;
  const allProfiles =
    profilesContext &&
    isAgentSettingsContextCurrent(profilesContext, activeRequestContext.current)
      ? loadedProfiles
      : [];
  const profiles = allProfiles.filter(
    (profile) => profile.provider === provider
  );
  const copyTargetProfiles =
    profileAction?.kind === 'copy'
      ? allProfiles.filter(
          (profile) => profile.provider === profileAction.targetProvider
        )
      : [];
  const copyPreview =
    scopedCopyPreview &&
    isAgentSettingsContextCurrent(
      scopedCopyPreview.context,
      activeRequestContext.current
    )
      ? scopedCopyPreview.preview
      : null;

  const clearSensitiveVisibility = useCallback(() => {
    revealRequestSequence.current += 1;
    setRevealedSetting(null);
    setVisibleReplacementIds({});
    setRevealingSettingId(null);
    setRevealErrors({});
  }, []);

  const hideSensitiveSetting = useCallback((id: string) => {
    revealRequestSequence.current += 1;
    setRevealedSetting((current) => (current?.id === id ? null : current));
    setVisibleReplacementIds((current) => {
      if (!current[id]) return current;
      const next = { ...current };
      delete next[id];
      return next;
    });
    setRevealingSettingId((current) => (current === id ? null : current));
    setRevealErrors((current) => {
      if (!current[id]) return current;
      const next = { ...current };
      delete next[id];
      return next;
    });
  }, []);

  useEffect(() => {
    clearSensitiveVisibility();
  }, [
    activeSection,
    clearSensitiveVisibility,
    machineClient,
    projectPath,
    provider,
    scope,
  ]);

  const loadSettings = useCallback(async () => {
    if (!machineClient) return;
    const requestContext = { ...activeRequestContext.current };
    const sequence = ++settingsRequestSequence.current;
    setLoadedSnapshot(null);
    setSnapshotContext(null);
    setDraft({});
    setLoading(true);
    setError(null);
    try {
      const inventory = await machineClient.discoverAgentSettings({
        provider,
        project_path: requestContext.projectPath || null,
      });
      if (
        !isAgentSettingsRequestCurrent({
          requestSequence: sequence,
          currentSequence: settingsRequestSequence.current,
          requestContext,
          currentContext: activeRequestContext.current,
        })
      ) {
        return;
      }
      const next =
        inventory.providers.find(
          (candidate) => candidate.provider === provider
        ) ?? null;
      setLoadedSnapshot(next);
      setSnapshotContext(requestContext);
      setDraft(next ? createAgentSettingsDraft(next) : {});
      setProfileEnvironmentRaw(
        formatProfileEnvironment(profileEnvironmentFromSnapshot(next))
      );
      setProfileCustomArgsRaw('');
      setProfileFieldsDirty(false);
      setProfileEditor(null);
      setDiff(null);
      setPending(null);
      if (inventory.errors.length > 0 && !next) {
        setError(inventory.errors.map((item) => item.message).join(' '));
      }
    } catch (nextError) {
      if (
        isAgentSettingsRequestCurrent({
          requestSequence: sequence,
          currentSequence: settingsRequestSequence.current,
          requestContext,
          currentContext: activeRequestContext.current,
        })
      ) {
        setError(formatAgentSettingOperationError(nextError));
        setLoadedSnapshot(null);
        setSnapshotContext(null);
        setDraft({});
        setProfileEnvironmentRaw('{}');
        setProfileCustomArgsRaw('');
        setProfileFieldsDirty(false);
        setProfileEditor(null);
      }
    } finally {
      if (
        isAgentSettingsRequestCurrent({
          requestSequence: sequence,
          currentSequence: settingsRequestSequence.current,
          requestContext,
          currentContext: activeRequestContext.current,
        })
      ) {
        setLoading(false);
      }
    }
  }, [machineClient, provider]);

  const loadProfiles = useCallback(async () => {
    if (!machineClient) return;
    const requestContext = { ...activeRequestContext.current };
    const sequence = ++profileRequestSequence.current;
    setLoadedProfiles([]);
    setProfilesContext(null);
    setProfilesLoading(true);
    try {
      const nextProfiles = await machineClient.listAgentSettingsProfiles();
      if (
        isAgentSettingsRequestCurrent({
          requestSequence: sequence,
          currentSequence: profileRequestSequence.current,
          requestContext,
          currentContext: activeRequestContext.current,
        })
      ) {
        setLoadedProfiles(nextProfiles);
        setProfilesContext(requestContext);
      }
    } catch (nextError) {
      if (
        isAgentSettingsRequestCurrent({
          requestSequence: sequence,
          currentSequence: profileRequestSequence.current,
          requestContext,
          currentContext: activeRequestContext.current,
        })
      ) {
        setError(formatAgentSettingOperationError(nextError));
      }
    } finally {
      if (
        isAgentSettingsRequestCurrent({
          requestSequence: sequence,
          currentSequence: profileRequestSequence.current,
          requestContext,
          currentContext: activeRequestContext.current,
        })
      ) {
        setProfilesLoading(false);
      }
    }
  }, [machineClient]);

  useEffect(() => {
    setActiveSection('overview');
  }, [machineClient, provider]);

  useEffect(() => {
    settingsRequestSequence.current += 1;
    profileRequestSequence.current += 1;
    operationRequestSequence.current += 1;
    setLoadedSnapshot(null);
    setSnapshotContext(null);
    setDraft({});
    setLoadedProfiles([]);
    setProfilesContext(null);
    setLoading(Boolean(machineClient));
    setProfilesLoading(Boolean(machineClient));
    setBusy(false);
    setDiff(null);
    setPending(null);
    setScopedCopyPreview(null);
    setProfileEditor(null);
    setProfileAction(null);
    setError(null);
    setNotice(null);
    void loadSettings();
    void loadProfiles();
  }, [loadProfiles, loadSettings, machineClient, projectPath, provider, scope]);

  const sections = useMemo(() => {
    const nextSections = buildAgentSettingsSections(snapshot);
    return nextSections.filter(
      (section) => includeTools || section.id !== 'tools'
    );
  }, [includeTools, snapshot]);

  useEffect(() => {
    if (
      sections.length > 0 &&
      !sections.some((section) => section.id === activeSection)
    ) {
      setActiveSection(sections[0].id);
    }
  }, [activeSection, sections]);

  useEffect(() => {
    if (!projectPath.trim() && scope === SettingScope.project) {
      setScope(SettingScope.user);
    }
  }, [projectPath, scope]);

  const projectScopeSupported = Boolean(
    snapshot?.descriptors.some((descriptor) =>
      descriptor.supported_scopes.includes(SettingScope.project)
    )
  );

  useEffect(() => {
    if (!projectScopeSupported && scope === SettingScope.project) {
      setScope(SettingScope.user);
    }
  }, [projectScopeSupported, scope]);

  const draftDirty = isAgentSettingsDraftDirty(draft);
  const profileEnvironment = useMemo(
    () => parseProfileEnvironment(profileEnvironmentRaw),
    [profileEnvironmentRaw]
  );
  const profileEditorEnvironment = useMemo(
    () =>
      profileEditor
        ? parseProfileEnvironment(profileEditor.environmentRaw)
        : { value: {} as Record<string, string> },
    [profileEditor]
  );
  const profileEditorDirty = useMemo(() => {
    if (!profileEditor) return false;
    return (
      profileEditor.name !== profileEditor.initialName ||
      profileEditor.environmentRaw !== profileEditor.initialEnvironmentRaw ||
      profileEditor.customArgsRaw !== profileEditor.initialCustomArgsRaw
    );
  }, [profileEditor]);
  const profileActionDirty = Boolean(
    profileAction &&
      (profileAction.name !== profileAction.initialName ||
        (profileAction.kind === 'copy' &&
          (profileAction.targetProvider !==
            profileAction.initialTargetProvider ||
            profileAction.targetProfileId !==
              profileAction.initialTargetProfileId)))
  );
  const isDirty =
    draftDirty ||
    profileFieldsDirty ||
    profileEditorDirty ||
    profileActionDirty;

  useEffect(() => {
    setContextDirty(`agent-settings:${provider}`, isDirty);
    return () => setContextDirty(`agent-settings:${provider}`, false);
  }, [isDirty, provider, setContextDirty]);

  const applySnapshot = (next: SettingsSnapshot) => {
    setLoadedSnapshot(next);
    setSnapshotContext({ ...activeRequestContext.current });
    setDraft(createAgentSettingsDraft(next));
    setDiff(null);
    setPending(null);
  };

  const beginOperation = (): AgentSettingsOperation | null => {
    const client = machineClient;
    const context = { ...activeRequestContext.current };
    // A controlled confirmation can outlive the render that opened it. Never
    // issue its mutation through that render's old MachineClient after the
    // selected Host or another request scope has changed.
    if (!client || context.client !== client) return null;
    return {
      sequence: ++operationRequestSequence.current,
      context,
      client,
    };
  };

  const operationIsCurrent = (
    operation: AgentSettingsOperation | null
  ): operation is AgentSettingsOperation => {
    if (!operation) return false;
    return isAgentSettingsRequestCurrent({
      requestSequence: operation.sequence,
      currentSequence: operationRequestSequence.current,
      requestContext: operation.context,
      currentContext: activeRequestContext.current,
    });
  };

  const patch = useMemo(
    () =>
      snapshot
        ? buildAgentSettingsPatch(
            snapshot,
            draft,
            scope,
            projectPath.trim() || undefined
          )
        : null,
    [draft, projectPath, scope, snapshot]
  );

  const previewSettings = async () => {
    if (!machineClient || !patch || !snapshot) return;
    if (hasDraftErrors(draft)) {
      setError({ key: 'invalidSettings' });
      return;
    }
    if (patch.operations.length === 0) {
      setNotice('noChanges');
      return;
    }
    const operation = beginOperation();
    if (!operation) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const nextDiff = await operation.client.diffAgentSettings(patch);
      if (!operationIsCurrent(operation)) return;
      setDiff(nextDiff);
      setPending({ kind: 'settings', patch, context: operation.context });
    } catch (nextError) {
      if (operationIsCurrent(operation)) {
        setError(formatAgentSettingOperationError(nextError));
      }
    } finally {
      if (operationIsCurrent(operation)) setBusy(false);
    }
  };

  const confirmPending = async () => {
    if (!machineClient || !pending) return;
    if (
      !isAgentSettingsContextCurrent(
        pending.context,
        activeRequestContext.current
      )
    ) {
      setDiff(null);
      setPending(null);
      return;
    }
    const operation = beginOperation();
    if (!operation) return;
    setBusy(true);
    setError(null);
    try {
      let nextSnapshot: SettingsSnapshot;
      if (pending.kind === 'settings') {
        nextSnapshot = await operation.client.applyAgentSettings({
          patch: pending.patch,
          confirmed: true,
        });
      } else {
        nextSnapshot = await operation.client.applyAgentSettingsProfile({
          preview: pending.preview,
          confirmed: true,
        });
      }
      if (!operationIsCurrent(operation)) return;
      applySnapshot(nextSnapshot);
      await loadProfiles();
      if (!operationIsCurrent(operation)) return;
      setNotice('applied');
    } catch (nextError) {
      if (operationIsCurrent(operation)) {
        setError(formatAgentSettingOperationError(nextError));
      }
    } finally {
      if (operationIsCurrent(operation)) setBusy(false);
    }
  };

  const discardDraft = () => {
    if (snapshot) setDraft(createAgentSettingsDraft(snapshot));
    setDiff(null);
    setPending(null);
    setNotice(null);
    setError(null);
  };

  const discardAllLocalChanges = () => {
    discardDraft();
    setProfileEnvironmentRaw(
      formatProfileEnvironment(profileEnvironmentFromSnapshot(snapshot))
    );
    setProfileCustomArgsRaw('');
    setProfileFieldsDirty(false);
    setProfileEditor(null);
    setProfileAction(null);
    setScopedCopyPreview(null);
  };

  const updateDraft = (
    descriptor: SettingDescriptor,
    raw: string,
    value?: JsonValue,
    parseError?: string
  ) => {
    const id = settingKeyId(descriptor);
    setDraft((previous) => ({
      ...previous,
      [id]: {
        ...previous[id],
        action:
          raw.length === 0 && descriptor.sensitive ? 'preserve' : 'replace',
        raw,
        value,
        error: parseError,
      },
    }));
    setDiff(null);
    setPending(null);
  };

  const resetDraft = (descriptor: SettingDescriptor) => {
    const id = settingKeyId(descriptor);
    hideSensitiveSetting(id);
    setDraft((previous) => ({
      ...previous,
      [id]: {
        ...previous[id],
        action: 'clear',
        raw: '',
        value: undefined,
        error: undefined,
      },
    }));
    setDiff(null);
    setPending(null);
  };

  const revealSensitiveSetting = async (descriptor: SettingDescriptor) => {
    if (!machineClient || !snapshot) return;
    const id = settingKeyId(descriptor);
    const source = settingSourceForScope(snapshot, descriptor, scope);
    if (!source?.configured) return;
    const client = machineClient;
    const requestContext = { ...activeRequestContext.current };
    const expectedRevision = source.revision;
    const sequence = ++revealRequestSequence.current;
    setRevealedSetting(null);
    setRevealingSettingId(id);
    setRevealErrors((current) => {
      if (!current[id]) return current;
      const next = { ...current };
      delete next[id];
      return next;
    });
    try {
      const result = await client.revealAgentSetting({
        provider,
        project_path: projectPath.trim() || null,
        key: descriptor.key,
        scope,
        expected_revision: expectedRevision,
      });
      if (
        !isAgentSettingsRequestCurrent({
          requestSequence: sequence,
          currentSequence: revealRequestSequence.current,
          requestContext,
          currentContext: activeRequestContext.current,
        }) ||
        !isRevealResponseCurrent({
          requestSequence: sequence,
          currentSequence: revealRequestSequence.current,
          requestClient: client,
          currentClient: activeMachineClient.current,
          settingId: id,
          scope,
          expectedRevision,
          response: result,
        })
      ) {
        return;
      }
      setRevealedSetting({ id, value: result.value });
    } catch (nextError) {
      if (
        isAgentSettingsRequestCurrent({
          requestSequence: sequence,
          currentSequence: revealRequestSequence.current,
          requestContext,
          currentContext: activeRequestContext.current,
        }) &&
        activeMachineClient.current === client
      ) {
        setRevealErrors((current) => ({
          ...current,
          [id]: formatAgentSettingOperationError(nextError),
        }));
      }
    } finally {
      if (
        isAgentSettingsRequestCurrent({
          requestSequence: sequence,
          currentSequence: revealRequestSequence.current,
          requestContext,
          currentContext: activeRequestContext.current,
        }) &&
        activeMachineClient.current === client
      ) {
        setRevealingSettingId(null);
      }
    }
  };

  const saveProfile = async (name: string) => {
    if (!machineClient || !snapshot) return;
    if (!profileEnvironment.value) {
      setError(profileEnvironment.error ?? { key: 'invalidEnvironment' });
      return;
    }
    if (!name.trim()) return;
    const settingOverrides: Record<string, JsonValue> = {};
    for (const descriptor of snapshot.descriptors) {
      if (!descriptor.capabilities.profile_storable) continue;
      const entry = draft[settingKeyId(descriptor)];
      if (entry?.action === 'clear') continue;
      if (descriptor.sensitive && entry?.action !== 'replace') continue;
      const effectiveValue = snapshot.effective_settings.find(
        (candidate) => settingKeyId(candidate) === settingKeyId(descriptor)
      )?.effective_value;
      const value =
        entry?.action === 'replace' && entry.value !== undefined
          ? entry.value
          : effectiveValue;
      if (value !== undefined)
        settingOverrides[settingKeyId(descriptor)] = value;
    }
    const operation = beginOperation();
    if (!operation) return;
    setBusy(true);
    setError(null);
    try {
      await operation.client.saveAgentSettingsProfile({
        profile: {
          id: newProfileId(),
          provider,
          executor_profile: {
            executor,
            variant: variant === 'DEFAULT' ? null : variant,
          },
          name: name.trim(),
          schema_version: 1,
          setting_overrides: settingOverrides,
          provider_extensions: {},
          environment: profileEnvironment.value,
          custom_args: parseProfileCustomArgs(profileCustomArgsRaw),
          updated_at: new Date().toISOString(),
        },
      });
      if (!operationIsCurrent(operation)) return;
      await loadProfiles();
      if (!operationIsCurrent(operation)) return;
      setProfileFieldsDirty(false);
      setProfileAction(null);
      setNotice('profileSaved');
    } catch (nextError) {
      if (operationIsCurrent(operation)) {
        setError(formatAgentSettingOperationError(nextError));
      }
    } finally {
      if (operationIsCurrent(operation)) setBusy(false);
    }
  };

  const editProfile = (profile: ConfigProfileView) => {
    const environmentRaw = formatProfileEnvironment(profile.environment);
    const customArgsRaw = profile.custom_args.join('\n');
    setProfileEditor({
      id: profile.id,
      name: profile.name,
      environmentRaw,
      customArgsRaw,
      initialName: profile.name,
      initialEnvironmentRaw: environmentRaw,
      initialCustomArgsRaw: customArgsRaw,
    });
  };

  const saveProfileEdits = async (profile: ConfigProfileView) => {
    if (
      !machineClient ||
      !profileEditor ||
      profileEditor.id !== profile.id ||
      !profileEditor.name.trim()
    ) {
      return;
    }
    if (!profileEditorEnvironment.value) {
      setError(profileEditorEnvironment.error ?? { key: 'invalidEnvironment' });
      return;
    }
    const operation = beginOperation();
    if (!operation) return;
    setBusy(true);
    setError(null);
    try {
      await operation.client.updateAgentSettingsProfile({
        id: profile.id,
        name: profileEditor.name.trim(),
        environment: profileEditorEnvironment.value,
        custom_args: parseProfileCustomArgs(profileEditor.customArgsRaw),
      });
      if (!operationIsCurrent(operation)) return;
      await loadProfiles();
      if (!operationIsCurrent(operation)) return;
      setProfileEditor(null);
      setNotice('profileUpdated');
    } catch (nextError) {
      if (operationIsCurrent(operation)) {
        setError(formatAgentSettingOperationError(nextError));
      }
    } finally {
      if (operationIsCurrent(operation)) setBusy(false);
    }
  };

  const duplicateProfile = async (profileId: string, name: string) => {
    if (!machineClient) return;
    if (!name.trim()) return;
    const operation = beginOperation();
    if (!operation) return;
    setBusy(true);
    setError(null);
    try {
      await operation.client.duplicateAgentSettingsProfile({
        id: profileId,
        name: name.trim(),
      });
      if (!operationIsCurrent(operation)) return;
      await loadProfiles();
      if (!operationIsCurrent(operation)) return;
      setProfileAction(null);
      setNotice('profileDuplicated');
    } catch (nextError) {
      if (operationIsCurrent(operation)) {
        setError(formatAgentSettingOperationError(nextError));
      }
    } finally {
      if (operationIsCurrent(operation)) setBusy(false);
    }
  };

  const deleteProfile = async (profile: ConfigProfileView) => {
    if (!machineClient) return;
    const confirmationContext = { ...activeRequestContext.current };
    const result = await ConfirmDialog.show({
      title: tc('deleteTitle'),
      message: tc('deleteMessage', { name: profile.name }),
      confirmText: tc('delete'),
      cancelText: t('buttons.cancel'),
      variant: 'destructive',
    });
    if (result !== 'confirmed') return;
    if (
      !isAgentSettingsContextCurrent(
        confirmationContext,
        activeRequestContext.current
      )
    ) {
      return;
    }
    const operation = beginOperation();
    if (!operation) return;
    setBusy(true);
    setError(null);
    try {
      await operation.client.deleteAgentSettingsProfile({ id: profile.id });
      if (!operationIsCurrent(operation)) return;
      await loadProfiles();
      if (!operationIsCurrent(operation)) return;
      setNotice('profileDeleted');
    } catch (nextError) {
      if (operationIsCurrent(operation)) {
        setError(formatAgentSettingOperationError(nextError));
      }
    } finally {
      if (operationIsCurrent(operation)) setBusy(false);
    }
  };

  const previewProfileApply = async (profile: ConfigProfileView) => {
    if (!machineClient || !snapshot) return;
    const expected_file_revisions = Object.fromEntries(
      snapshot.native_files.map((file) => [
        file.file_id,
        nativeFileRevision(file),
      ])
    );
    const request: ProfileApplyPreviewRequest = {
      id: profile.id,
      project_path: projectPath.trim() || null,
      scope,
      expected_file_revisions,
    };
    const operation = beginOperation();
    if (!operation) return;
    setBusy(true);
    setError(null);
    try {
      const nextDiff =
        await operation.client.previewAgentSettingsProfileApply(request);
      if (!operationIsCurrent(operation)) return;
      setDiff(nextDiff);
      setPending({
        kind: 'profile',
        preview: request,
        context: operation.context,
      });
    } catch (nextError) {
      if (operationIsCurrent(operation)) {
        setError(formatAgentSettingOperationError(nextError));
      }
    } finally {
      if (operationIsCurrent(operation)) setBusy(false);
    }
  };

  const previewProfileCopy = async (
    profileId: string,
    targetProvider: AgentSettingsProvider,
    targetName: string,
    targetProfileId: string | null
  ) => {
    if (!machineClient) return;
    if (!targetProfileId && !targetName.trim()) return;
    const operation = beginOperation();
    if (!operation) return;
    setBusy(true);
    setError(null);
    try {
      const request: CopyProfilePreviewRequest = {
        id: profileId,
        target_provider: targetProvider,
        target_executor_profile: {
          executor: executorForProvider(targetProvider),
          variant: null,
        },
        target_name: targetName.trim(),
        target_profile_id: targetProfileId,
      };
      const preview =
        await operation.client.previewAgentSettingsProfileCopy(request);
      if (!operationIsCurrent(operation)) return;
      setScopedCopyPreview({ preview, request, context: operation.context });
      setProfileAction(null);
    } catch (nextError) {
      if (operationIsCurrent(operation)) {
        setError(formatAgentSettingOperationError(nextError));
      }
    } finally {
      if (operationIsCurrent(operation)) setBusy(false);
    }
  };

  const saveCopiedProfile = async () => {
    if (!machineClient || !copyPreview) return;
    const operation = beginOperation();
    if (!operation || !scopedCopyPreview) return;
    if (
      !isAgentSettingsContextCurrent(
        scopedCopyPreview.context,
        operation.context
      )
    ) {
      setScopedCopyPreview(null);
      return;
    }
    setBusy(true);
    try {
      await operation.client.copyAgentSettingsProfile({
        preview: scopedCopyPreview.request,
        expected_source_updated_at: copyPreview.source_updated_at,
        expected_target_updated_at: copyPreview.target_updated_at,
        confirmed: true,
      });
      if (!operationIsCurrent(operation)) return;
      await loadProfiles();
      if (!operationIsCurrent(operation)) return;
      setScopedCopyPreview(null);
      setNotice(copyPreview.target_updated_at ? 'copyUpdated' : 'copySaved');
    } catch (nextError) {
      if (operationIsCurrent(operation)) {
        setError(formatAgentSettingOperationError(nextError));
      }
    } finally {
      if (operationIsCurrent(operation)) setBusy(false);
    }
  };

  const confirmLocalDiscard = async (): Promise<boolean> => {
    if (!isDirty) return true;
    const result = await ConfirmDialog.show({
      title: t('agentCenter.unsaved.title'),
      message: t('agentCenter.unsaved.message'),
      confirmText: t('agentCenter.unsaved.discard'),
      cancelText: t('agentCenter.unsaved.cancel'),
      variant: 'destructive',
    });
    return result === 'confirmed';
  };

  const refreshSettings = async () => {
    if (!(await confirmLocalDiscard())) return;
    if (isDirty) discardAllLocalChanges();
    await loadSettings();
  };

  const changeActiveSection = async (nextSection: string) => {
    if (nextSection === activeSection) return;
    if (!(await confirmLocalDiscard())) return;
    if (isDirty) discardAllLocalChanges();
    setActiveSection(nextSection);
  };

  const discoverProjectPath = async () => {
    const nextPath = projectPathInput.trim();
    if (nextPath === projectPath) {
      await refreshSettings();
      return;
    }
    if (!(await confirmLocalDiscard())) return;
    setProjectPath(nextPath);
  };

  const changeScope = async (nextScope: SettingScope) => {
    if (nextScope === scope || !(await confirmLocalDiscard())) return;
    setScope(nextScope);
  };

  const requestCloseProfileEditor = async () => {
    if (profileEditorDirty) {
      const result = await ConfirmDialog.show({
        title: tc('discardTitle'),
        message: tc('discardMessage'),
        confirmText: t('agentCenter.unsaved.discard'),
        cancelText: t('agentCenter.unsaved.cancel'),
        variant: 'destructive',
      });
      if (result !== 'confirmed') return;
    }
    setProfileEditor(null);
  };

  const requestCloseProfileAction = async () => {
    if (profileActionDirty) {
      const result = await ConfirmDialog.show({
        title: tc('discardTitle'),
        message: tc('discardDialogMessage'),
        confirmText: t('agentCenter.unsaved.discard'),
        cancelText: t('agentCenter.unsaved.cancel'),
        variant: 'destructive',
      });
      if (result !== 'confirmed') return;
    }
    setProfileAction(null);
  };

  const submitProfileAction = async () => {
    if (!profileAction) return;
    if (profileAction.kind !== 'copy' && !profileAction.name.trim()) {
      return;
    }
    if (profileAction.kind === 'create') {
      await saveProfile(profileAction.name);
    } else if (profileAction.kind === 'duplicate') {
      await duplicateProfile(profileAction.sourceId, profileAction.name);
    } else {
      await previewProfileCopy(
        profileAction.sourceId,
        profileAction.targetProvider,
        profileAction.name,
        profileAction.targetProfileId
      );
    }
  };

  const renderOverview = () => (
    <SettingsCard
      title={tc('overviewTitle', { provider: PROVIDER_LABELS[provider] })}
      description={tc('overviewDescription')}
      headerAction={
        <PrimaryButton
          variant="tertiary"
          value={tc('refresh')}
          onClick={() => void refreshSettings()}
          disabled={loading || !machineClient}
          actionIcon={loading ? 'spinner' : undefined}
        />
      }
    >
      <div className="grid gap-3 md:grid-cols-2">
        <InfoTile
          label={tc('installed')}
          value={snapshot?.installed ? tc('yes') : tc('notDetected')}
        />
        <InfoTile
          label={tc('version')}
          value={snapshot?.provider_version ?? tc('unknown')}
        />
      </div>
      <SettingsField
        label={tc('projectPath')}
        description={tc('projectPathHelp')}
      >
        <div className="flex gap-2">
          <SettingsInput
            value={formatPathForDisplay(projectPathInput)}
            onChange={setProjectPathInput}
            placeholder="/absolute/path/to/project"
            disabled={busy}
          />
          <button
            type="button"
            className="rounded-sm border border-border px-3 text-low hover:text-normal disabled:opacity-40"
            aria-label={tc('discoverProject')}
            disabled={busy || loading}
            onClick={() => void discoverProjectPath()}
          >
            <ArrowClockwiseIcon className="size-icon-xs" aria-hidden="true" />
          </button>
        </div>
      </SettingsField>
      <SettingsField label={tc('scope')} description={tc('scopeHelp')}>
        <select
          value={scope}
          onChange={(event) =>
            void changeScope(event.target.value as SettingScope)
          }
          disabled={busy}
          className="w-full rounded-sm border border-border bg-secondary px-base py-half text-sm text-high focus:outline-none focus:ring-1 focus:ring-brand"
        >
          <option value={SettingScope.user}>{tc('userScope')}</option>
          <option
            value={SettingScope.project}
            disabled={!projectPath.trim() || !projectScopeSupported}
          >
            {tc('projectScope')}
          </option>
        </select>
      </SettingsField>
      <div className="flex flex-wrap gap-2" aria-label={tc('capabilities')}>
        {snapshot &&
          Object.entries(snapshot.capabilities).map(([key, enabled]) => (
            <span
              key={key}
              className="rounded bg-secondary px-2 py-1 text-xs text-low"
            >
              {tc(`capability.${key}`, { defaultValue: key })}:{' '}
              {enabled ? tc('available') : tc('unsupported')}
            </span>
          ))}
      </div>
      {snapshot && !snapshot.installed && (
        <div className="flex items-start gap-2 rounded-sm border border-border bg-secondary/30 p-3 text-sm text-low">
          <WarningCircleIcon
            className="mt-0.5 size-icon-sm shrink-0"
            aria-hidden="true"
          />
          {tc('notInstalled')}
        </div>
      )}
      {snapshot?.limitations.map((limitation) => (
        <div
          key={limitation}
          className="rounded-sm border border-border bg-secondary/20 p-3 text-xs text-low"
        >
          {limitation}
        </div>
      ))}
      {snapshot?.errors.map((item) => (
        <div
          key={`${item.file_id ?? ''}:${item.setting_key ?? ''}:${item.message}`}
          className="rounded-sm border border-error/40 bg-error/10 p-3 text-sm text-error"
          role="alert"
        >
          {item.message} {item.recovery}
        </div>
      ))}
    </SettingsCard>
  );

  const renderSettings = (descriptors: SettingDescriptor[]) => (
    <SettingsCard
      title={tc(`sections.${activeSection}`, { defaultValue: tc('settings') })}
      description={tc('settingsHelp')}
    >
      <div className="space-y-5">
        {descriptors.map((descriptor) => {
          const id = settingKeyId(descriptor);
          const entry = draft[id] ?? { action: 'preserve' as const, raw: '' };
          const resettable = descriptor.capabilities.resettable;
          const scopeSupported = descriptor.supported_scopes.includes(scope);
          const disabled =
            !descriptor.capabilities.writable || busy || !scopeSupported;
          const selectValue =
            entry.value === undefined ? '' : JSON.stringify(entry.value);
          const update = (raw: string, nextValue?: JsonValue) => {
            const parsed = parseDraftChange(descriptor, raw, nextValue);
            updateDraft(descriptor, raw, parsed.value, parsed.error);
          };
          if (descriptor.sensitive) {
            const configured = Boolean(
              snapshot &&
                settingSourceForScope(snapshot, descriptor, scope)?.configured
            );
            const viewingStoredValue = revealedSetting?.id === id;
            const replacementVisible = Boolean(visibleReplacementIds[id]);
            const valueVisible = viewingStoredValue || replacementVisible;
            const canToggleVisibility =
              (entry.action === 'preserve' && configured) ||
              (entry.action === 'replace' && entry.raw.length > 0);
            const revealLoading = revealingSettingId === id;
            return (
              <SettingsField
                key={id}
                label={descriptor.label}
                description={descriptor.description}
                error={entry.error ?? revealErrors[id]}
              >
                <div className="flex items-center justify-between gap-2 text-xs text-low">
                  <span>
                    {configured
                      ? t('agentCenter.security.sensitiveValueConfigured')
                      : t('agentCenter.security.sensitiveValueNotConfigured')}
                  </span>
                  {configured && descriptor.capabilities.resettable && (
                    <button
                      type="button"
                      className="underline hover:text-normal disabled:opacity-40"
                      disabled={disabled}
                      onClick={() => {
                        hideSensitiveSetting(id);
                        resetDraft(descriptor);
                      }}
                    >
                      {t('agentCenter.security.clearSensitiveValue')}
                    </button>
                  )}
                </div>
                <div className="relative">
                  <input
                    id={`agent-setting-${id.replaceAll('.', '-')}`}
                    type={valueVisible ? 'text' : 'password'}
                    value={
                      viewingStoredValue ? revealedSetting.value : entry.raw
                    }
                    onChange={(event) => {
                      hideSensitiveSetting(id);
                      update(event.target.value);
                    }}
                    placeholder={
                      configured && entry.action === 'preserve'
                        ? t(
                            'agentCenter.security.sensitiveValueConfiguredPlaceholder'
                          )
                        : undefined
                    }
                    disabled={disabled}
                    readOnly={viewingStoredValue}
                    autoComplete="new-password"
                    className={cn(
                      'w-full rounded-sm border border-border bg-secondary px-base py-half pr-10 font-mono text-sm text-high',
                      'placeholder:text-low placeholder:opacity-80 focus:outline-none focus:ring-1 focus:ring-brand',
                      disabled && 'cursor-not-allowed opacity-50'
                    )}
                  />
                  <button
                    type="button"
                    className="absolute right-1 top-1/2 flex size-8 -translate-y-1/2 items-center justify-center rounded-sm text-low hover:bg-panel hover:text-normal disabled:cursor-not-allowed disabled:opacity-40"
                    aria-label={t(
                      valueVisible
                        ? 'agentCenter.security.hideSensitiveValue'
                        : 'agentCenter.security.showSensitiveValue'
                    )}
                    disabled={
                      busy ||
                      !scopeSupported ||
                      revealLoading ||
                      !canToggleVisibility
                    }
                    onClick={() => {
                      if (valueVisible) {
                        hideSensitiveSetting(id);
                      } else if (entry.action === 'replace') {
                        setVisibleReplacementIds((current) => ({
                          ...current,
                          [id]: true,
                        }));
                      } else {
                        void revealSensitiveSetting(descriptor);
                      }
                    }}
                  >
                    {revealLoading ? (
                      <SpinnerIcon
                        className="size-icon-sm animate-spin"
                        aria-hidden="true"
                      />
                    ) : valueVisible ? (
                      <EyeSlashIcon
                        className="size-icon-sm"
                        aria-hidden="true"
                      />
                    ) : (
                      <EyeIcon className="size-icon-sm" aria-hidden="true" />
                    )}
                  </button>
                </div>
                <div className="text-xs text-low">
                  {t('agentCenter.security.writeOnlySettingHelp')}
                </div>
              </SettingsField>
            );
          }
          return (
            <SettingsField
              key={id}
              label={descriptor.label}
              description={descriptor.description}
              error={entry.error}
            >
              <div className="flex items-center justify-between gap-2">
                <span className="text-xs text-low">
                  <span className="rounded bg-secondary px-1.5 py-0.5">
                    {sourceLabel(sourceLabelKind(snapshot!, descriptor, entry))}
                  </span>
                  <span className="ml-2">
                    {tc(`activation.${descriptor.activation}`)}
                  </span>
                </span>
                {resettable && (
                  <button
                    type="button"
                    className="text-xs text-low underline hover:text-normal disabled:opacity-40"
                    disabled={disabled}
                    onClick={() => resetDraft(descriptor)}
                  >
                    {tc('restore')}
                  </button>
                )}
              </div>
              {!scopeSupported && (
                <div className="text-xs text-low">
                  {tc('scopeUnavailable', {
                    scope: tc(
                      scope === SettingScope.user ? 'userScope' : 'projectScope'
                    ),
                  })}
                </div>
              )}
              {descriptor.control === 'toggle' ? (
                <div className="flex items-center gap-3">
                  <Switch
                    checked={
                      entry.action === 'clear' ? false : Boolean(entry.value)
                    }
                    disabled={disabled}
                    aria-label={descriptor.label}
                    onCheckedChange={(checked) =>
                      update(checked ? 'true' : 'false', checked)
                    }
                  />
                  <span className="text-sm text-normal">
                    {entry.value ? tc('enabled') : tc('disabled')}
                  </span>
                </div>
              ) : descriptor.control === 'select' ? (
                <select
                  value={selectValue}
                  disabled={disabled}
                  aria-label={descriptor.label}
                  onChange={(event) => update(event.target.value)}
                  className="w-full rounded-sm border border-border bg-secondary px-base py-half text-sm text-high focus:outline-none focus:ring-1 focus:ring-brand"
                >
                  <option value="">{tc('followNative')}</option>
                  {descriptor.options.map((option) => (
                    <option
                      key={JSON.stringify(option.value)}
                      value={JSON.stringify(option.value)}
                    >
                      {option.label}
                    </option>
                  ))}
                </select>
              ) : descriptor.control === 'textarea' ||
                descriptor.control === 'string_list' ||
                descriptor.control === 'key_value' ||
                descriptor.control === 'json' ? (
                <SettingsTextarea
                  value={entry.raw}
                  onChange={(raw) => update(raw)}
                  disabled={disabled}
                  rows={
                    descriptor.control === 'json' ||
                    descriptor.control === 'key_value'
                      ? 5
                      : 3
                  }
                  monospace={
                    descriptor.control === 'json' ||
                    descriptor.control === 'key_value'
                  }
                />
              ) : (
                <SettingsInput
                  value={entry.raw}
                  onChange={(raw) => update(raw)}
                  disabled={disabled}
                  error={Boolean(entry.error)}
                />
              )}
            </SettingsField>
          );
        })}
      </div>
      <SettingsSaveBar
        show={draftDirty}
        saving={busy}
        saveDisabled={
          hasDraftErrors(draft) || !snapshot?.capabilities.native_writable
        }
        onSave={() => void previewSettings()}
        onDiscard={discardDraft}
      />
    </SettingsCard>
  );

  const renderEffective = () => (
    <SettingsCard
      title={tc('effectiveTitle')}
      description={tc('effectiveHelp')}
    >
      <div className="overflow-x-auto rounded-sm border border-border">
        <table className="w-full text-left text-xs">
          <thead className="bg-secondary/50 text-low">
            <tr>
              <th className="px-3 py-2">{tc('setting')}</th>
              <th className="px-3 py-2">{tc('effective')}</th>
              <th className="px-3 py-2">{tc('source')}</th>
              <th className="px-3 py-2">{tc('observedSources')}</th>
            </tr>
          </thead>
          <tbody>
            {snapshot?.effective_settings.map((setting) => (
              <tr
                key={settingKeyId(setting)}
                className="border-t border-border/60 align-top"
              >
                <td className="px-3 py-2 font-mono text-normal">
                  {settingKeyId(setting)}
                </td>
                <td className="max-w-[18rem] whitespace-pre-wrap px-3 py-2 font-mono text-normal">
                  {snapshot.descriptors.some(
                    (descriptor) =>
                      settingKeyId(descriptor) === settingKeyId(setting) &&
                      descriptor.sensitive
                  )
                    ? t('agentCenter.security.effectiveValueHidden')
                    : displayJson(
                        setting.effective_value,
                        t('agentCenter.inheritedOrUnset')
                      )}
                </td>
                <td className="px-3 py-2 text-low">
                  {sourceLabel(
                    agentSettingSourceKind(setting.effective_source)
                  )}
                </td>
                <td className="px-3 py-2 text-low">
                  {setting.sources.length
                    ? setting.sources
                        .map(
                          (source) =>
                            `${sourceLabel(agentSettingSourceKind(source.source))} (${source.scope})`
                        )
                        .join(', ')
                    : t('agentCenter.inheritedOrUnset')}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {snapshot?.unknown_native_nodes.length ? (
        <details className="rounded-sm border border-border bg-secondary/20 p-3">
          <summary className="cursor-pointer text-sm text-normal">
            {tc('unknownNodes', {
              count: snapshot.unknown_native_nodes.length,
            })}
          </summary>
          <div className="mt-3 space-y-2">
            {snapshot.unknown_native_nodes.map((node) => (
              <div
                key={`${node.file_id}:${node.field_path}`}
                className="font-mono text-xs text-low"
              >
                {node.file_id}:{node.field_path} ={' '}
                {t('agentCenter.security.unknownValueHidden')}
              </div>
            ))}
          </div>
        </details>
      ) : null}
    </SettingsCard>
  );

  const renderNativeConfig = () => {
    const files = snapshot ? buildNativeConfigFileModels(snapshot, scope) : [];
    return (
      <SettingsCard
        title={t('agentCenter.nativeConfig.title')}
        description={t('agentCenter.nativeConfig.description')}
        headerAction={
          <select
            value={scope}
            onChange={(event) =>
              void changeScope(event.target.value as SettingScope)
            }
            disabled={busy}
            aria-label={t('agentCenter.nativeConfig.scopeLabel')}
            className="rounded-sm border border-border bg-secondary px-3 py-2 text-sm text-normal focus:outline-none focus:ring-1 focus:ring-brand"
          >
            <option value={SettingScope.user}>
              {t('agentCenter.nativeConfig.userScope')}
            </option>
            {projectScopeSupported && projectPath.trim() ? (
              <option value={SettingScope.project}>
                {t('agentCenter.nativeConfig.projectScope')}
              </option>
            ) : null}
          </select>
        }
      >
        <div
          className="rounded-sm border border-border bg-secondary/20 p-3 text-sm text-low"
          role="note"
        >
          {t('agentCenter.nativeConfig.safeEditingHelp')}
        </div>
        {files.length === 0 ? (
          <div className="rounded-sm border border-dashed border-border p-4 text-sm text-low">
            {t('agentCenter.nativeConfig.noFiles')}
          </div>
        ) : (
          <div className="space-y-4">
            {files.map(({ file, descriptors, unknownNodes }) => (
              <section
                key={`${file.scope}:${file.file_id}`}
                className="space-y-3 rounded-sm border border-border bg-secondary/10 p-4"
                aria-labelledby={`native-config-${file.scope}-${file.file_id}`}
              >
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0">
                    <h4
                      id={`native-config-${file.scope}-${file.file_id}`}
                      className="font-mono text-sm font-medium text-high"
                    >
                      {file.file_id}
                    </h4>
                    <div className="mt-1 flex flex-wrap gap-1.5 text-xs text-low">
                      <span className="rounded bg-secondary px-2 py-1">
                        {file.format.toUpperCase()}
                      </span>
                      <span className="rounded bg-secondary px-2 py-1">
                        {t(
                          `agentCenter.nativeConfig.parseStatus.${file.parse_status}`
                        )}
                      </span>
                      <span className="rounded bg-secondary px-2 py-1">
                        {t(
                          file.writable
                            ? 'agentCenter.nativeConfig.writable'
                            : 'agentCenter.nativeConfig.readOnly'
                        )}
                      </span>
                    </div>
                  </div>
                  <div className="max-w-full text-right text-xs text-low">
                    <div>{t('agentCenter.nativeConfig.revision')}</div>
                    <code className="block max-w-72 truncate text-normal">
                      {nativeFileRevision(file)}
                    </code>
                  </div>
                </div>

                <div>
                  <div className="mb-2 text-xs font-medium uppercase tracking-wide text-low">
                    {t('agentCenter.nativeConfig.managedFields')}
                  </div>
                  {descriptors.length === 0 ? (
                    <div className="text-sm text-low">
                      {t('agentCenter.nativeConfig.noManagedFields')}
                    </div>
                  ) : (
                    <div className="divide-y divide-border rounded-sm border border-border bg-background/30">
                      {descriptors.map((descriptor) => {
                        const id = settingKeyId(descriptor);
                        const source = snapshot
                          ? settingSourceForScope(snapshot, descriptor, scope)
                          : null;
                        const canEdit =
                          file.writable &&
                          descriptor.capabilities.writable &&
                          descriptor.supported_scopes.includes(scope) &&
                          file.parse_status !== 'invalid' &&
                          file.parse_status !== 'unsupported';
                        return (
                          <div
                            key={id}
                            className="flex flex-wrap items-center justify-between gap-3 px-3 py-2"
                          >
                            <div className="min-w-0">
                              <div className="text-sm text-normal">
                                {descriptor.label}
                              </div>
                              <div className="font-mono text-xs text-low">
                                {id} ·{' '}
                                {t(
                                  source?.configured
                                    ? 'agentCenter.nativeConfig.configured'
                                    : 'agentCenter.nativeConfig.inherited'
                                )}
                              </div>
                            </div>
                            {canEdit ? (
                              <Button
                                type="button"
                                variant="outline"
                                onClick={() =>
                                  void changeActiveSection(descriptor.section)
                                }
                              >
                                {t('agentCenter.nativeConfig.editField')}
                              </Button>
                            ) : (
                              <span className="text-xs text-low">
                                {t('agentCenter.nativeConfig.notEditable')}
                              </span>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>

                {unknownNodes.length > 0 ? (
                  <details className="rounded-sm border border-border bg-background/20 p-3">
                    <summary className="cursor-pointer text-sm text-normal">
                      {t('agentCenter.nativeConfig.unknownFields', {
                        count: unknownNodes.length,
                      })}
                    </summary>
                    <div className="mt-3 space-y-2">
                      {unknownNodes.map((node) => (
                        <div
                          key={`${node.file_id}:${node.field_path}`}
                          className="flex flex-wrap items-center justify-between gap-2 font-mono text-xs text-low"
                        >
                          <span>{node.field_path}</span>
                          <span>
                            {node.value_kind} ·{' '}
                            {t('agentCenter.nativeConfig.valueHidden')}
                          </span>
                        </div>
                      ))}
                    </div>
                  </details>
                ) : null}
              </section>
            ))}
          </div>
        )}
      </SettingsCard>
    );
  };

  const renderProfiles = () => (
    <SettingsCard
      title={tc('profilesTitle')}
      description={tc('profilesHelp')}
      headerAction={
        <PrimaryButton
          value={tc('saveCurrent')}
          onClick={() =>
            setProfileAction({ kind: 'create', name: '', initialName: '' })
          }
          disabled={
            busy ||
            !snapshot?.capabilities.profile_storage ||
            Boolean(profileEnvironment.error)
          }
          actionIcon={busy ? 'spinner' : undefined}
        />
      }
    >
      <div className="grid gap-4 md:grid-cols-2">
        <SettingsField
          label={t('agentCenter.profileEditor.environmentLabel')}
          description={t('agentCenter.profileEditor.environmentHelp')}
          error={profileEnvironment.error}
        >
          <SettingsTextarea
            value={profileEnvironmentRaw}
            onChange={(value) => {
              setProfileEnvironmentRaw(value);
              setProfileFieldsDirty(true);
            }}
            placeholder={t('agentCenter.profileEditor.environmentPlaceholder')}
            disabled={busy}
            rows={5}
            monospace
          />
        </SettingsField>
        <SettingsField
          label={t('agentCenter.profileEditor.customArgsLabel')}
          description={t('agentCenter.profileEditor.customArgsHelp')}
        >
          <SettingsTextarea
            value={profileCustomArgsRaw}
            onChange={(value) => {
              setProfileCustomArgsRaw(value);
              setProfileFieldsDirty(true);
            }}
            placeholder={t('agentCenter.profileEditor.customArgsPlaceholder')}
            disabled={busy}
            rows={5}
            monospace
          />
        </SettingsField>
      </div>
      {copyPreview && (
        <div className="space-y-2 rounded-sm border border-border bg-secondary/20 p-3">
          <div className="text-sm font-medium text-high">
            {t('agentCenter.profileEditor.copy.previewTitle', {
              provider: PROVIDER_LABELS[copyPreview.profile.provider],
            })}
          </div>
          <div className="text-xs text-low">
            {t('agentCenter.profileEditor.copy.addLabel')}:{' '}
            {copyPreview.added_keys.length
              ? copyPreview.added_keys.join(', ')
              : t('agentCenter.profileEditor.copy.none')}
          </div>
          <div className="text-xs text-low">
            {t('agentCenter.profileEditor.copy.overwriteLabel')}:{' '}
            {copyPreview.overwritten_keys.length
              ? copyPreview.overwritten_keys.join(', ')
              : t('agentCenter.profileEditor.copy.none')}
          </div>
          <div className="text-xs text-low">
            {t('agentCenter.profileEditor.copy.skippedLabel')}:{' '}
            {copyPreview.skipped_keys.length
              ? copyPreview.skipped_keys.join(', ')
              : t('agentCenter.profileEditor.copy.none')}
          </div>
          {copyPreview.warnings.map((warning) => (
            <div key={warning} className="text-xs text-error">
              {warning}
            </div>
          ))}
          <div className="flex justify-end gap-2">
            <PrimaryButton
              variant="tertiary"
              value={t('buttons.cancel')}
              onClick={() => setScopedCopyPreview(null)}
              disabled={busy}
            />
            <PrimaryButton
              value={
                copyPreview.target_updated_at
                  ? t('agentCenter.profileEditor.copy.updateAction')
                  : t('agentCenter.profileEditor.copy.createAction')
              }
              onClick={() => void saveCopiedProfile()}
              disabled={busy}
              actionIcon={busy ? 'spinner' : undefined}
            />
          </div>
        </div>
      )}
      {profilesLoading ? (
        <div
          className="flex items-center gap-2 py-4 text-sm text-low"
          role="status"
          aria-live="polite"
        >
          <SpinnerIcon className="size-icon-xs animate-spin" />{' '}
          {tc('loadingProfiles')}
        </div>
      ) : profiles.length === 0 ? (
        <div className="rounded-sm border border-dashed border-border p-4 text-sm text-low">
          {tc('noProfiles')}
        </div>
      ) : (
        <div className="space-y-2">
          {profiles.map((profile) => {
            const editing = profileEditor?.id === profile.id;
            return (
              <div
                key={profile.id}
                className="space-y-3 rounded-sm border border-border bg-secondary/20 px-3 py-2"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="min-w-0">
                    <div className="text-sm font-medium text-normal">
                      {profile.name}
                    </div>
                    <div className="text-xs text-low">
                      {tc('profileSummary', {
                        settings:
                          Object.keys(profile.setting_overrides).length +
                          profile.configured_credential_keys.length,
                        environment: Object.keys(profile.environment).length,
                        args: profile.custom_args.length,
                        date: new Intl.DateTimeFormat(
                          i18n.resolvedLanguage ?? 'en',
                          { dateStyle: 'medium', timeStyle: 'short' }
                        ).format(new Date(profile.updated_at)),
                      })}
                    </div>
                  </div>
                  <div className="flex flex-wrap gap-1">
                    <ToolAction
                      label={tc('apply')}
                      icon={CheckCircleIcon}
                      disabled={busy || !snapshot || profileEditor !== null}
                      onClick={() => void previewProfileApply(profile)}
                    />
                    <ToolAction
                      label={tc('copy')}
                      icon={CopyIcon}
                      disabled={busy || profileEditor !== null}
                      onClick={() => {
                        const targetProvider = (
                          Object.keys(
                            PROVIDER_LABELS
                          ) as AgentSettingsProvider[]
                        ).find((candidate) => candidate !== profile.provider);
                        if (!targetProvider) return;
                        const name = `${profile.name} (${PROVIDER_LABELS[targetProvider]})`;
                        setProfileAction({
                          kind: 'copy',
                          sourceId: profile.id,
                          sourceName: profile.name,
                          name,
                          initialName: name,
                          targetProvider,
                          initialTargetProvider: targetProvider,
                          targetProfileId: null,
                          initialTargetProfileId: null,
                        });
                      }}
                    />
                    <ToolAction
                      label={tc('duplicate')}
                      icon={PlusIcon}
                      disabled={busy || profileEditor !== null}
                      onClick={() => {
                        const name = tc('copyName', { name: profile.name });
                        setProfileAction({
                          kind: 'duplicate',
                          sourceId: profile.id,
                          sourceName: profile.name,
                          name,
                          initialName: name,
                        });
                      }}
                    />
                    <ToolAction
                      label={t('agentCenter.profileEditor.edit')}
                      icon={PencilSimpleIcon}
                      disabled={busy || profileEditor !== null}
                      onClick={() => editProfile(profile)}
                    />
                    <ToolAction
                      label={tc('delete')}
                      icon={TrashIcon}
                      danger
                      disabled={busy || profileEditor !== null}
                      onClick={() => void deleteProfile(profile)}
                    />
                  </div>
                </div>
                {editing && profileEditor && (
                  <div className="space-y-4 border-t border-border pt-3">
                    <SettingsField
                      label={t('agentCenter.profileEditor.nameLabel')}
                    >
                      <SettingsInput
                        value={profileEditor.name}
                        onChange={(name) =>
                          setProfileEditor((current) =>
                            current ? { ...current, name } : current
                          )
                        }
                        disabled={busy}
                      />
                    </SettingsField>
                    <div className="grid gap-4 md:grid-cols-2">
                      <SettingsField
                        label={t('agentCenter.profileEditor.environmentLabel')}
                        description={t(
                          'agentCenter.profileEditor.environmentHelp'
                        )}
                        error={profileEditorEnvironment.error}
                      >
                        <SettingsTextarea
                          value={profileEditor.environmentRaw}
                          onChange={(environmentRaw) =>
                            setProfileEditor((current) =>
                              current ? { ...current, environmentRaw } : current
                            )
                          }
                          disabled={busy}
                          rows={5}
                          monospace
                        />
                      </SettingsField>
                      <SettingsField
                        label={t('agentCenter.profileEditor.customArgsLabel')}
                        description={t(
                          'agentCenter.profileEditor.customArgsHelp'
                        )}
                      >
                        <SettingsTextarea
                          value={profileEditor.customArgsRaw}
                          onChange={(customArgsRaw) =>
                            setProfileEditor((current) =>
                              current ? { ...current, customArgsRaw } : current
                            )
                          }
                          disabled={busy}
                          rows={5}
                          monospace
                        />
                      </SettingsField>
                    </div>
                    <div className="flex justify-end gap-2">
                      <PrimaryButton
                        variant="tertiary"
                        value={t('agentCenter.profileEditor.cancel')}
                        onClick={() => void requestCloseProfileEditor()}
                        disabled={busy}
                      />
                      <PrimaryButton
                        value={t('agentCenter.profileEditor.save')}
                        onClick={() => void saveProfileEdits(profile)}
                        disabled={
                          busy ||
                          !profileEditorDirty ||
                          !profileEditor.name.trim() ||
                          Boolean(profileEditorEnvironment.error)
                        }
                        actionIcon={busy ? 'spinner' : undefined}
                      />
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </SettingsCard>
  );

  if (!machineClient)
    return <div className="py-4 text-sm text-low">{tc('selectMachine')}</div>;
  if (loading && !snapshot)
    return (
      <div
        className="flex items-center gap-2 py-6 text-sm text-low"
        role="status"
        aria-live="polite"
      >
        <SpinnerIcon className="size-icon-sm animate-spin" />
        {tc('discovering', { provider: PROVIDER_LABELS[provider] })}
      </div>
    );

  const active = sections.find((section) => section.id === activeSection);
  return (
    <>
      <div
        className="mt-5 space-y-4 border-t border-border pt-5"
        data-testid="agent-settings-panel"
      >
        <div className="flex items-center gap-2">
          <CodeIcon className="size-icon-sm text-brand" aria-hidden="true" />
          <h3 className="text-base font-medium text-high">
            {tc('installedSettings')}
          </h3>
        </div>
        {error && (
          <div
            className="flex items-start gap-2 rounded-sm border border-error/50 bg-error/10 p-3 text-sm text-error"
            role="alert"
          >
            <WarningCircleIcon className="mt-0.5 size-icon-sm shrink-0" />
            <div className="min-w-0 space-y-2">
              <p>{tc('operationFailed')}</p>
              <details>
                <summary className="cursor-pointer">{tc('details')}</summary>
                <p className="mt-2 whitespace-pre-wrap break-words">
                  {typeof error === 'string' ? error : tc(error.key)}
                </p>
              </details>
            </div>
          </div>
        )}
        {notice && (
          <div
            className="flex items-start gap-2 rounded-sm border border-success/50 bg-success/10 p-3 text-sm text-success"
            role="status"
          >
            <CheckCircleIcon className="mt-0.5 size-icon-sm shrink-0" />
            {notice === 'copyUpdated'
              ? t('agentCenter.profileEditor.copy.updateNotice')
              : notice === 'copySaved'
                ? t('agentCenter.profileEditor.copy.createNotice')
                : tc(notice)}
          </div>
        )}
        {!snapshot ? (
          <div className="rounded-sm border border-border p-4 text-sm text-low">
            {tc('noSnapshot')}
          </div>
        ) : (
          <>
            <div
              className="flex flex-wrap gap-1 border-b border-border"
              role="tablist"
              aria-label={tc('sectionsLabel')}
            >
              {sections.map((section) => (
                <button
                  key={section.id}
                  type="button"
                  role="tab"
                  aria-selected={activeSection === section.id}
                  onClick={() => void changeActiveSection(section.id)}
                  className={cn(
                    'border-b-2 px-3 py-2 text-sm',
                    activeSection === section.id
                      ? 'border-brand text-brand'
                      : 'border-transparent text-low hover:text-normal'
                  )}
                >
                  {section.id === 'native_config'
                    ? t('agentCenter.nativeConfig.sectionLabel')
                    : tc(`sections.${section.id}`, {
                        defaultValue: section.label,
                      })}
                </button>
              ))}
            </div>
            {activeSection === 'overview' && renderOverview()}
            {active?.descriptors.length
              ? renderSettings(active.descriptors)
              : null}
            {includeTools && activeSection === 'tools' && (
              <AgentToolsSettingsSection provider={provider} />
            )}
            {activeSection === 'profiles' && renderProfiles()}
            {activeSection === 'native_config' && renderNativeConfig()}
            {activeSection === 'effective_config' && renderEffective()}
          </>
        )}
        {diff && (
          <DiffConfirmation
            diff={diff}
            pending={pending}
            busy={busy}
            onCancel={() => {
              setDiff(null);
              setPending(null);
            }}
            onConfirm={() => void confirmPending()}
          />
        )}
        <SettingsSaveBar
          show={draftDirty && active?.descriptors.length === 0}
          saving={busy}
          onSave={() => void previewSettings()}
          onDiscard={discardDraft}
        />
      </div>
      <Dialog
        open={profileAction !== null}
        uncloseable={busy}
        onOpenChange={(open) => {
          if (!open && !busy) void requestCloseProfileAction();
        }}
      >
        <DialogContent className="sm:max-w-[480px]">
          {profileAction && (
            <form
              onSubmit={(event) => {
                event.preventDefault();
                void submitProfileAction();
              }}
            >
              <DialogHeader>
                <DialogTitle>
                  {profileAction.kind === 'create'
                    ? tc('saveProfileTitle')
                    : profileAction.kind === 'duplicate'
                      ? tc('duplicateProfileTitle')
                      : tc('copyProfileTitle')}
                </DialogTitle>
                <DialogDescription className="text-left">
                  {profileAction.kind === 'create'
                    ? tc('saveProfileHelp')
                    : tc('sourceProfile', { name: profileAction.sourceName })}
                </DialogDescription>
              </DialogHeader>
              <div className="space-y-4 py-4">
                {profileAction.kind === 'copy' && (
                  <label className="block space-y-2 text-sm font-medium text-normal">
                    <span>
                      {t('agentCenter.profileEditor.copy.targetProviderLabel')}
                    </span>
                    <select
                      value={profileAction.targetProvider}
                      onChange={(event) => {
                        const targetProvider = event.target
                          .value as AgentSettingsProvider;
                        setProfileAction((current) =>
                          current?.kind === 'copy'
                            ? {
                                ...current,
                                targetProvider,
                                targetProfileId: null,
                              }
                            : current
                        );
                      }}
                      disabled={busy}
                      className="min-h-11 w-full rounded-sm border border-border bg-panel px-3 text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
                    >
                      {(Object.keys(PROVIDER_LABELS) as AgentSettingsProvider[])
                        .filter((candidate) => candidate !== provider)
                        .map((candidate) => (
                          <option key={candidate} value={candidate}>
                            {PROVIDER_LABELS[candidate]}
                          </option>
                        ))}
                    </select>
                    <span className="block text-xs font-normal text-low">
                      {t('agentCenter.profileEditor.copy.providerNotice')}
                    </span>
                  </label>
                )}
                {profileAction.kind === 'copy' && (
                  <label className="block space-y-2 text-sm font-medium text-normal">
                    <span>
                      {t('agentCenter.profileEditor.copy.destinationLabel')}
                    </span>
                    <select
                      value={profileAction.targetProfileId ?? ''}
                      onChange={(event) => {
                        const targetProfileId = event.target.value || null;
                        setProfileAction((current) =>
                          current?.kind === 'copy'
                            ? { ...current, targetProfileId }
                            : current
                        );
                      }}
                      disabled={busy}
                      className="min-h-11 w-full rounded-sm border border-border bg-panel px-3 text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
                    >
                      <option value="">
                        {t('agentCenter.profileEditor.copy.createDestination')}
                      </option>
                      {copyTargetProfiles.map((profile) => (
                        <option key={profile.id} value={profile.id}>
                          {t(
                            'agentCenter.profileEditor.copy.updateDestination',
                            { name: profile.name }
                          )}
                        </option>
                      ))}
                    </select>
                    <span className="block text-xs font-normal text-low">
                      {t('agentCenter.profileEditor.copy.existingTargetHelp')}
                    </span>
                  </label>
                )}
                {(profileAction.kind !== 'copy' ||
                  profileAction.targetProfileId === null) && (
                  <label className="block space-y-2 text-sm font-medium text-normal">
                    <span>{t('agentCenter.profileEditor.nameLabel')}</span>
                    <input
                      value={profileAction.name}
                      onChange={(event) => {
                        const name = event.target.value;
                        setProfileAction((current) =>
                          current ? { ...current, name } : current
                        );
                      }}
                      autoFocus
                      disabled={busy}
                      className="min-h-11 w-full rounded-sm border border-border bg-panel px-3 text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
                    />
                  </label>
                )}
              </div>
              <DialogFooter>
                <Button
                  type="button"
                  variant="outline"
                  disabled={busy}
                  onClick={() => void requestCloseProfileAction()}
                >
                  {t('buttons.cancel')}
                </Button>
                <Button
                  type="submit"
                  disabled={
                    busy ||
                    (profileAction.kind !== 'copy' &&
                      !profileAction.name.trim()) ||
                    (profileAction.kind === 'copy' &&
                      profileAction.targetProfileId === null &&
                      !profileAction.name.trim())
                  }
                >
                  {busy && (
                    <SpinnerIcon
                      className="mr-2 size-icon-xs animate-spin"
                      aria-hidden="true"
                    />
                  )}
                  {profileAction.kind === 'copy'
                    ? t('agentCenter.profileEditor.copy.previewAction')
                    : t('agentCenter.profileEditor.save')}
                </Button>
              </DialogFooter>
            </form>
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}

function InfoTile({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="rounded-sm border border-border bg-secondary/20 p-3">
      <div className="text-xs text-low">{label}</div>
      <div
        className={cn('mt-1 truncate text-sm text-normal', mono && 'font-mono')}
      >
        {value}
      </div>
    </div>
  );
}

function DiffConfirmation({
  diff,
  pending,
  busy,
  onCancel,
  onConfirm,
}: {
  diff: SettingsDiff;
  pending: PendingAction | null;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation('common');
  return (
    <div
      className="space-y-3 rounded-sm border border-brand/40 bg-brand/5 p-3"
      role="dialog"
      aria-label={t('agentCenter.settingsDiff.ariaLabel')}
    >
      <div className="flex items-center gap-2 text-sm font-medium text-high">
        <FloppyDiskIcon className="size-icon-sm text-brand" />
        {t('agentCenter.settingsDiff.title')}
      </div>
      {diff.warnings.map((warning) => (
        <div key={warning} className="text-xs text-error">
          {warning}
        </div>
      ))}
      {diff.files
        .filter((file) => file.changed)
        .map((file) => (
          <details
            key={file.file_id}
            open
            className="rounded-sm border border-border bg-background/40"
          >
            <summary className="cursor-pointer px-3 py-2 text-sm text-normal">
              {t('agentCenter.settingsDiff.fileLabel', {
                fileId: file.file_id,
                format: file.format.toUpperCase(),
                scope: file.scope,
              })}
            </summary>
            <div className="border-t border-border p-3 text-xs text-low">
              {t('agentCenter.security.diffHidden')}
            </div>
          </details>
        ))}
      {!hasChangedFiles(diff) && (
        <div className="text-sm text-low">
          {t('agentCenter.settingsDiff.noChanges')}
        </div>
      )}
      <div className="flex justify-end gap-2">
        <PrimaryButton
          variant="tertiary"
          value={t('buttons.cancel')}
          onClick={onCancel}
          disabled={busy}
        />
        <PrimaryButton
          value={
            pending?.kind === 'profile'
              ? t('agentCenter.settingsDiff.confirmProfile')
              : t('agentCenter.settingsDiff.confirm')
          }
          onClick={onConfirm}
          disabled={busy || !hasChangedFiles(diff)}
          actionIcon={busy ? 'spinner' : undefined}
        />
      </div>
    </div>
  );
}

function ToolAction({
  label,
  icon: Icon,
  onClick,
  disabled,
  danger,
}: {
  label: string;
  icon: typeof PlusIcon;
  onClick: () => void;
  disabled?: boolean;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={cn(
        'inline-flex items-center gap-1 rounded-sm px-2 py-1 text-xs',
        danger
          ? 'text-error hover:bg-error/10'
          : 'text-low hover:bg-secondary hover:text-normal',
        disabled && 'cursor-not-allowed opacity-40'
      )}
    >
      <Icon className="size-icon-2xs" aria-hidden="true" />
      {label}
    </button>
  );
}
