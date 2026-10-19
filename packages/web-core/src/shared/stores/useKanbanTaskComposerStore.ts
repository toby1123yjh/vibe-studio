import { useCallback } from 'react';
import { create } from 'zustand';
import type { TaskPriority } from 'shared/remote-types';

export interface ProjectTaskCreateOptions {
  statusId?: string;
  priority?: TaskPriority;
  assigneeIds?: string[];
  parentTaskId?: string;
}

export interface KanbanTaskComposerDraft {
  title: string;
  description: string | null;
  statusId?: string;
  priority?: TaskPriority | null;
  assigneeIds?: string[];
  tagIds?: string[];
  parentTaskId?: string;
}

export interface KanbanTaskComposerEntry {
  initial: KanbanTaskComposerDraft;
  draft: KanbanTaskComposerDraft;
}

interface KanbanTaskComposerState {
  byKey: Record<string, KanbanTaskComposerEntry | undefined>;
  openComposer: (
    key: string,
    options?: ProjectTaskCreateOptions | null
  ) => void;
  patchComposer: (key: string, patch: Partial<KanbanTaskComposerDraft>) => void;
  resetComposer: (key: string) => void;
  closeComposer: (key: string) => void;
}

const LOCAL_HOST_SCOPE = 'local';

function normalizeComposerDraft(
  draft: Partial<KanbanTaskComposerDraft>
): KanbanTaskComposerDraft {
  return {
    title: draft.title ?? '',
    description: draft.description ?? null,
    ...(draft.statusId ? { statusId: draft.statusId } : {}),
    ...(draft.priority !== undefined ? { priority: draft.priority } : {}),
    ...(draft.assigneeIds !== undefined
      ? { assigneeIds: [...draft.assigneeIds] }
      : {}),
    ...(draft.tagIds !== undefined ? { tagIds: [...draft.tagIds] } : {}),
    ...(draft.parentTaskId ? { parentTaskId: draft.parentTaskId } : {}),
  };
}

export function buildKanbanTaskComposerKey(
  hostId: string | null,
  projectId: string
): string {
  const hostScope = hostId ?? LOCAL_HOST_SCOPE;
  return `${hostScope}:${projectId}`;
}

function toInitialComposerDraft(
  options?: ProjectTaskCreateOptions | null
): KanbanTaskComposerDraft {
  return normalizeComposerDraft({
    statusId: options?.statusId,
    priority: options?.priority,
    assigneeIds: options?.assigneeIds,
    parentTaskId: options?.parentTaskId,
    tagIds: [],
  });
}

export const useKanbanTaskComposerStore = create<KanbanTaskComposerState>()(
  (set) => ({
    byKey: {},
    openComposer: (key, options) =>
      set((state) => {
        const initial = toInitialComposerDraft(options);
        return {
          byKey: {
            ...state.byKey,
            [key]: {
              initial,
              draft: initial,
            },
          },
        };
      }),
    patchComposer: (key, patch) =>
      set((state) => {
        const current = state.byKey[key];
        if (!current) {
          return state;
        }

        return {
          byKey: {
            ...state.byKey,
            [key]: {
              ...current,
              draft: normalizeComposerDraft({
                ...current.draft,
                ...patch,
              }),
            },
          },
        };
      }),
    resetComposer: (key) =>
      set((state) => {
        const current = state.byKey[key];
        if (!current) {
          return state;
        }

        return {
          byKey: {
            ...state.byKey,
            [key]: {
              ...current,
              draft: current.initial,
            },
          },
        };
      }),
    closeComposer: (key) =>
      set((state) => {
        if (!(key in state.byKey)) {
          return state;
        }

        const rest = { ...state.byKey };
        delete rest[key];
        return { byKey: rest };
      }),
  })
);

export function useKanbanTaskComposer(
  composerKey: string | null
): KanbanTaskComposerEntry | null {
  return useKanbanTaskComposerStore(
    useCallback(
      (state) => (composerKey ? (state.byKey[composerKey] ?? null) : null),
      [composerKey]
    )
  );
}

export function openKanbanTaskComposer(
  composerKey: string,
  options?: ProjectTaskCreateOptions | null
): void {
  useKanbanTaskComposerStore.getState().openComposer(composerKey, options);
}

export function patchKanbanTaskComposer(
  composerKey: string,
  patch: Partial<KanbanTaskComposerDraft>
): void {
  useKanbanTaskComposerStore.getState().patchComposer(composerKey, patch);
}

export function resetKanbanTaskComposer(composerKey: string): void {
  useKanbanTaskComposerStore.getState().resetComposer(composerKey);
}

export function closeKanbanTaskComposer(composerKey: string): void {
  useKanbanTaskComposerStore.getState().closeComposer(composerKey);
}
