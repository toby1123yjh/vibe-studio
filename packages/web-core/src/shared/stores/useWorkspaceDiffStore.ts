import { create } from 'zustand';
import {
  createContext,
  createElement,
  useContext,
  useMemo,
  type ReactNode,
} from 'react';
import type { Diff, DiffStats, UnifiedPrComment } from 'shared/types';
import type { NormalizedGitHubComment } from '@/shared/hooks/useWorkspaceContext';

// ---------------------------------------------------------------------------
// Zustand store for workspace diff data (diffs, stats, GitHub comments).
// Populated by WorkspaceProvider via setWorkspaceDiffData(); consumers can
// subscribe to individual slices with the exported atomic selectors below.
// ---------------------------------------------------------------------------

const EMPTY_DIFFS: Diff[] = [];
const EMPTY_DIFF_PATHS: Set<string> = new Set();
const EMPTY_DIFF_STATS: DiffStats = {
  files_changed: 0,
  lines_added: 0,
  lines_removed: 0,
};
const EMPTY_COMMENTS: UnifiedPrComment[] = [];
const EMPTY_NORMALIZED: NormalizedGitHubComment[] = [];
const EMPTY_FILES: string[] = [];

const noopGetCommentsForFile = () => EMPTY_NORMALIZED;
const noopGetCommentCountForFile = () => 0;
const noopGetFilesWithComments = () => EMPTY_FILES;
const noopGetFirstCommentLine = () => null;
const noopSetShowGitHubComments = () => {};

// ---------------------------------------------------------------------------
// State shape
// ---------------------------------------------------------------------------

interface WorkspaceDiffData {
  diffs: Diff[];
  diffError: string | null;
  diffPaths: Set<string>;
  diffStats: DiffStats;
  gitHubComments: UnifiedPrComment[];
  isGitHubCommentsLoading: boolean;
  showGitHubComments: boolean;
  setShowGitHubComments: (show: boolean) => void;
  getGitHubCommentsForFile: (filePath: string) => NormalizedGitHubComment[];
  getGitHubCommentCountForFile: (filePath: string) => number;
  getFilesWithGitHubComments: () => string[];
  getFirstCommentLineForFile: (filePath: string) => number | null;
}

interface WorkspaceDiffState extends WorkspaceDiffData {
  /** Batch-update all diff data fields. Called by WorkspaceProvider. */
  setWorkspaceDiffData: (data: WorkspaceDiffData) => void;
  /** Reset to defaults. Called on workspace switch / unmount. */
  clearWorkspaceDiffData: () => void;
}

const DEFAULT_DATA: WorkspaceDiffData = {
  diffs: EMPTY_DIFFS,
  diffError: null,
  diffPaths: EMPTY_DIFF_PATHS,
  diffStats: EMPTY_DIFF_STATS,
  gitHubComments: EMPTY_COMMENTS,
  isGitHubCommentsLoading: false,
  showGitHubComments: false,
  setShowGitHubComments: noopSetShowGitHubComments,
  getGitHubCommentsForFile: noopGetCommentsForFile,
  getGitHubCommentCountForFile: noopGetCommentCountForFile,
  getFilesWithGitHubComments: noopGetFilesWithComments,
  getFirstCommentLineForFile: noopGetFirstCommentLine,
};

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

export const useWorkspaceDiffStore = create<WorkspaceDiffState>()((set) => ({
  ...DEFAULT_DATA,

  setWorkspaceDiffData: (data) => set(data),

  clearWorkspaceDiffData: () => set(DEFAULT_DATA),
}));

const WorkspaceDiffDataContext = createContext<WorkspaceDiffData | null>(null);

/** Bound tool views read their own diffs without changing the page's store. */
export function WorkspaceDiffDataProvider({
  value,
  children,
}: {
  value: Partial<WorkspaceDiffData>;
  children: ReactNode;
}) {
  const data = useMemo(() => ({ ...DEFAULT_DATA, ...value }), [value]);
  return createElement(
    WorkspaceDiffDataContext.Provider,
    { value: data },
    children
  );
}

function useWorkspaceDiffData<T>(selector: (data: WorkspaceDiffData) => T): T {
  const scopedData = useContext(WorkspaceDiffDataContext);
  return useWorkspaceDiffStore((data) => selector(scopedData ?? data));
}

// ---------------------------------------------------------------------------
// Atomic selectors — each subscribes to a single field to minimise rerenders
// ---------------------------------------------------------------------------

export const useDiffs = () => useWorkspaceDiffData((s) => s.diffs);

export const useDiffError = () => useWorkspaceDiffData((s) => s.diffError);

const WorkspaceDiffPathsContext = createContext<Set<string> | undefined>(
  undefined
);

/** A read-only scope for embedded messages; it never mutates the page store. */
export const WorkspaceDiffPathsProvider = WorkspaceDiffPathsContext.Provider;

export const useDiffPaths = (scopedPaths?: Set<string>) => {
  const contextPaths = useContext(WorkspaceDiffPathsContext);
  return useWorkspaceDiffData(
    (s) => scopedPaths ?? contextPaths ?? s.diffPaths
  );
};

export const useDiffStats = () => useWorkspaceDiffData((s) => s.diffStats);

export const useStoreDiffGitHubComments = () =>
  useWorkspaceDiffData((s) => s.gitHubComments);

export const useIsGitHubCommentsLoading = () =>
  useWorkspaceDiffData((s) => s.isGitHubCommentsLoading);

export const useShowGitHubComments = () =>
  useWorkspaceDiffData((s) => s.showGitHubComments);

export const useSetShowGitHubComments = () =>
  useWorkspaceDiffData((s) => s.setShowGitHubComments);

export const useGetGitHubCommentsForFile = () =>
  useWorkspaceDiffData((s) => s.getGitHubCommentsForFile);

export const useGetGitHubCommentCountForFile = () =>
  useWorkspaceDiffData((s) => s.getGitHubCommentCountForFile);

export const useGetFilesWithGitHubComments = () =>
  useWorkspaceDiffData((s) => s.getFilesWithGitHubComments);

export const useGetFirstCommentLineForFile = () =>
  useWorkspaceDiffData((s) => s.getFirstCommentLineForFile);
