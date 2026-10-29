import React, {
  forwardRef,
  useCallback,
  useImperativeHandle,
  useSyncExternalStore,
} from 'react';
import type { ComponentProps } from 'react';
import type { CreateChatBox as RealCreateChatBox } from '@vibe/ui/components/CreateChatBox';
import type { Scratch, UpdateScratch } from 'shared/types';

const params = new URLSearchParams(location.search);
const repo = {
  id: 'repo-1',
  name: 'project-repo',
  display_name: 'Project repo',
  path: '/project',
};
const listeners = new Set<() => void>();
let version = 0;
const notify = () => {
  version++;
  listeners.forEach((listener) => listener());
};
export function useFixture() {
  useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => version
  );
}
type ChatProps = ComponentProps<typeof RealCreateChatBox>;
type Submission = { data: Record<string, unknown>; linkToTask?: unknown };
interface FixtureState {
  composerVisible: boolean;
  repos: (typeof repo)[];
  targetBranches: Record<string, string>;
  directFolderPath: string;
  message: string;
  hasInitialValue: boolean;
  hasResolvedInitialWorkspaceDefaults: boolean;
  linkedTask: {
    remoteProjectId: string;
    taskId: string;
    simpleId: string;
  } | null;
  hostId: string;
}
export const fixture = {
  targetFailure: false,
  targetDeferred: false,
  resolveTarget: () => {},
  state: {
    composerVisible: true,
    repos: params.has('repo') ? [repo] : [],
    targetBranches: { 'repo-1': 'main' } as Record<string, string>,
    directFolderPath: params.get('path') ?? '',
    message: 'Hello agent',
    hasInitialValue: !params.has('loading'),
    hasResolvedInitialWorkspaceDefaults: !params.has('defaultsLoading'),
    linkedTask: params.has('project')
      ? { remoteProjectId: 'project-1', taskId: 'issue-1', simpleId: 'P-1' }
      : null,
    hostId: 'host-a',
  } as FixtureState,
  submissions: [] as Submission[],
  created: [] as string[],
  savedDefaults: [] as unknown[][],
  dialogCalls: [] as unknown[],
  clears: 0,
  pending: false,
  fail: false,
  error: null as Error | null,
  deferred: false,
  finish: () => {},
  resolveDialog: (_value: unknown) => {},
  chat: null as ChatProps | null,
  update(patch: Partial<FixtureState>) {
    Object.assign(this.state, patch);
    notify();
  },
  sendTwice() {
    void this.chat?.onSend(this.state.message);
    void this.chat?.onSend(this.state.message);
  },
};
Object.assign(window, { managedFixture: fixture });

const drafts = new Map<string, Scratch>();
export const draftTransport = {
  writes: [] as { scope: string; method: 'PUT' | 'DELETE'; message?: string }[],
  pending: [] as (() => void)[],
  release() {
    this.pending.shift()?.();
  },
};
Object.assign(window, { draftTransport });
export function useScratch(_type: unknown, id: string) {
  useFixture();
  const scope = `${fixture.state.hostId}:${id}`;
  const updateScratch = useCallback(
    async (update: UpdateScratch) => {
      draftTransport.writes.push({
        scope,
        method: 'PUT',
        message:
          update.payload.type === 'DRAFT_WORKSPACE'
            ? update.payload.data.message
            : undefined,
      });
      await new Promise<void>((resolve) => {
        draftTransport.pending.push(resolve);
      });
      drafts.set(scope, { payload: update.payload } as Scratch);
      notify();
    },
    [scope]
  );
  const deleteScratch = useCallback(async () => {
    draftTransport.writes.push({ scope, method: 'DELETE' });
    drafts.delete(scope);
    notify();
  }, [scope]);
  return {
    scratch: drafts.get(scope) ?? null,
    isLoading: false,
    updateScratch,
    deleteScratch,
  };
}
const emptyRepos: never[] = [];
export const useWorkspaceCreateDefaults = () => ({
  preferredRepos: emptyRepos,
  preferredExecutorConfig: null,
  hasResolvedPreferredRepos: true,
});
export const useShape = () => ({ data: emptyRepos });
const actions = {
  addRepo(value: typeof repo) {
    fixture.state.repos = [...fixture.state.repos, value];
    notify();
  },
  clearRepos() {
    fixture.state.repos = [];
    notify();
  },
  setTargetBranch(id: string, branch: string) {
    fixture.state.targetBranches = {
      ...fixture.state.targetBranches,
      [id]: branch,
    };
    notify();
  },
  setDirectFolderPath(value: string) {
    fixture.state.directFolderPath = value;
    notify();
  },
  setMessage(value: string) {
    fixture.state.message = value;
    notify();
  },
  clearDraft: async () => {
    fixture.clears++;
  },
  clearLinkedTask() {
    fixture.state.linkedTask = null;
    notify();
  },
  setExecutorConfig() {},
  setAttachments() {},
};
export function useCreateMode() {
  useFixture();
  return {
    ...fixture.state,
    initialProjectId: params.has('initialProject') ? 'project-1' : undefined,
    ...actions,
    attachments: [],
    executorConfig: null,
  };
}
const mutation = {
  get isPending() {
    return fixture.pending;
  },
  get error() {
    return fixture.error;
  },
  async mutateAsync(submission: Submission) {
    fixture.submissions.push(submission);
    fixture.pending = true;
    fixture.error = null;
    notify();
    if (fixture.deferred)
      await new Promise<void>((resolve) => {
        fixture.finish = resolve;
      });
    fixture.pending = false;
    if (fixture.fail) {
      fixture.error = new Error('creation failed');
      notify();
      throw fixture.error;
    }
    notify();
    return {
      workspace: { id: `workspace-${fixture.submissions.length}` },
      agentRun: {
        agent_run_id: `run-${fixture.submissions.length}`,
        state: { session_id: `session-${fixture.submissions.length}` },
      },
    };
  },
};
export function useCreateWorkspace() {
  useFixture();
  return {
    createWorkspace: {
      ...mutation,
      isPending: fixture.pending,
      error: fixture.error,
    },
  };
}
export const useCurrentAppDestination = () => ({
  kind: 'workspaces-create',
  hostId: fixture.state.hostId,
});
export const useUserSystem = () => ({ profiles: {}, config: {} });
const executorConfig = { executor: 'CODEX', variant: null };
export const useExecutorConfig = () => ({
  executorConfig,
  effectiveExecutor: 'CODEX',
  selectedVariant: null,
  executorOptions: [],
  variantOptions: [],
  presetOptions: [],
  setOverrides() {},
});
export const useCreateAttachments = () => ({
  uploadFiles() {},
  getAttachmentIds: () => [],
  clearAttachments() {},
  localAttachments: [],
});
export const saveProjectWorkspaceDefault = async (...args: unknown[]) => {
  fixture.savedDefaults.push(args);
  savedWorkspace = args[1];
};
export const getValidProjectWorkspaceDefault = async () => null;
let savedWorkspace: unknown = undefined;
export const getProjectWorkspaceDefaultOrThrow = async () => {
  if (fixture.targetDeferred)
    await new Promise<void>((resolve) => {
      fixture.resolveTarget = resolve;
    });
  if (fixture.targetFailure) throw new Error('workspace lookup failed');
  if (savedWorkspace !== undefined) return savedWorkspace;
  if (params.has('repo'))
    return { kind: 'git', repo: { repo_id: 'repo-1', target_branch: 'main' } };
  if (params.has('path'))
    return { kind: 'direct_folder', path: params.get('path') };
  return null;
};
export const useAppShellProjects = () => ({
  deployment: 'remote',
  hostId: fixture.state.hostId,
});
export const executionDataApi = {
  listProjects: async () => ({
    projects: [
      { id: '00000000-0000-0000-0000-000000000003', name: 'Default project' },
      { id: 'project-1', name: 'My project' },
    ],
    next_cursor: null,
  }),
};
export const repoApi = { getById: async () => repo };
export const useSettingsNavigation = () => ({ openAgentCenter() {} });
export const WorkspaceTargetDialog = {
  show: async (options: unknown) => {
    fixture.dialogCalls.push(options);
    return new Promise((resolve) => {
      fixture.resolveDialog = resolve;
    });
  },
};
const t = (key: string, options?: { defaultValue?: string }) =>
  options?.defaultValue ?? key;
export const useTranslation = () => ({ t });
export const useDropzone = () => ({
  getRootProps: () => ({}),
  getInputProps: () => ({}),
  isDragActive: false,
});
export const FolderOpenIcon = () => null;
export const AgentIcon = () => null;
export const getAgentName = (agent: string) => agent;
export const ModelSelectorContainer = () => null;
export const AgentSessionResumePicker = () => null;
export const AgentSessionResumeChip = () => null;
const Editor = forwardRef(function Editor(
  { value, disabled }: { value: string; disabled?: boolean },
  ref
) {
  useImperativeHandle(
    ref,
    () => ({
      getMarkdown: () => value,
      clear() {},
    }),
    [value]
  );
  return disabled ? <span>{value}</span> : null;
});
export default Editor;
// Deliberately thin view: real production container owns all target selection,
// initial-default hydration, request construction and asynchronous guards.
export function CreateChatBox(props: ChatProps) {
  fixture.chat = props;
  return (
    <section aria-label="Composer">
      <input
        aria-label="Message"
        value={props.editor.value}
        onChange={(event) => props.editor.onChange(event.target.value)}
      />
      <button
        disabled={props.disabled || props.isSending || props.sendDisabled}
        onClick={() => void props.onSend(props.editor.value)}
      >
        Send
      </button>
      {props.projectSelector}
      {props.linkedTask && (
        <div data-testid="linked-issue">
          {props.linkedTask.simpleId}
          {props.linkedTask.onRemove && (
            <button onClick={props.linkedTask.onRemove}>
              Remove Task link
            </button>
          )}
        </div>
      )}
      <output data-testid="error">{props.error}</output>
    </section>
  );
}
