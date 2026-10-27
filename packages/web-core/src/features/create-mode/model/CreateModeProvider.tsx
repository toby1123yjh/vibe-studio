import { useMemo, type ReactNode } from 'react';
import type { CreateModeInitialState } from '@/shared/types/createMode';
import { useCreateModeState } from '@/features/create-mode/model/useCreateModeState';
import { useWorkspaceContext } from '@/shared/hooks/useWorkspaceContext';
import { useCurrentAppDestination } from '@/shared/hooks/useCurrentAppDestination';
import { getDestinationHostId } from '@/shared/lib/routes/appNavigation';
import {
  CreateModeContext,
  type CreateModeContextValue,
} from '@/features/create-mode/model/useCreateMode';

interface CreateModeProviderProps {
  children: ReactNode;
  initialState?: CreateModeInitialState | null;
  draftId?: string | null;
}

export function CreateModeProvider({
  children,
  initialState,
  draftId,
}: CreateModeProviderProps) {
  // Reuse shell discovery when seeding project selection.
  const {
    activeWorkspaces,
    archivedWorkspaces,
    isWorkspacesListLoading: localWorkspacesLoading,
  } = useWorkspaceContext();
  const destination = useCurrentAppDestination();
  const hostId = useMemo(
    () => getDestinationHostId(destination),
    [destination]
  );
  const mostRecentWorkspace = activeWorkspaces[0] ?? archivedWorkspaces[0];
  const state = useCreateModeState({
    initialState,
    draftId,
    hostId,
    lastWorkspaceId: mostRecentWorkspace?.id ?? null,
    localWorkspacesLoading,
  });

  const value = useMemo<CreateModeContextValue>(
    () => ({
      initialProjectId: initialState?.project_id ?? undefined,
      repos: state.repos,
      addRepo: state.addRepo,
      removeRepo: state.removeRepo,
      clearRepos: state.clearRepos,
      targetBranches: state.targetBranches,
      setTargetBranch: state.setTargetBranch,
      directFolderPath: state.directFolderPath,
      setDirectFolderPath: state.setDirectFolderPath,
      hasResolvedInitialWorkspaceDefaults:
        state.hasResolvedInitialWorkspaceDefaults,
      preferredExecutorConfig: state.preferredExecutorConfig,
      message: state.message,
      setMessage: state.setMessage,
      clearDraft: state.clearDraft,
      hasInitialValue: state.hasInitialValue,
      linkedTask: state.linkedTask,
      clearLinkedTask: state.clearLinkedTask,
      executorConfig: state.executorConfig,
      setExecutorConfig: state.setExecutorConfig,
      attachments: state.attachments,
      setAttachments: state.setAttachments,
    }),
    [
      initialState?.project_id,
      state.repos,
      state.addRepo,
      state.removeRepo,
      state.clearRepos,
      state.targetBranches,
      state.setTargetBranch,
      state.directFolderPath,
      state.setDirectFolderPath,
      state.hasResolvedInitialWorkspaceDefaults,
      state.preferredExecutorConfig,
      state.message,
      state.setMessage,
      state.clearDraft,
      state.hasInitialValue,
      state.linkedTask,
      state.clearLinkedTask,
      state.executorConfig,
      state.setExecutorConfig,
      state.attachments,
      state.setAttachments,
    ]
  );

  return (
    <CreateModeContext.Provider value={value}>
      {children}
    </CreateModeContext.Provider>
  );
}
