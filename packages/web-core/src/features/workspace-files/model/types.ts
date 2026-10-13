import type {
  WorkspaceFileContent,
  WorkspaceFileDirectoryResponse,
  WorkspaceFileEntry,
  WorkspaceFileRepoNode,
  WorkspaceFileTreeResponse,
} from 'shared/types';

export type {
  WorkspaceFileContent,
  WorkspaceFileDirectoryResponse,
  WorkspaceFileEntry,
  WorkspaceFileRepoNode,
  WorkspaceFileTreeResponse,
};

export type WorkspaceFilePreviewSource =
  | 'file-tree'
  | 'chat'
  | 'diff'
  | 'workflow'
  | 'artifact';

export interface WorkspaceFilePreviewTarget {
  workspaceId: string;
  repoId: string;
  path: string;
  source: WorkspaceFilePreviewSource;
  sessionId?: string;
}

export interface WorkspaceFileTreeNode {
  id: string;
  repoId: string;
  repoName: string;
  name: string;
  path: string;
  type: 'file' | 'folder';
  entry: WorkspaceFileEntry;
  children?: WorkspaceFileTreeNode[];
  hasLoadedChildren?: boolean;
  truncated?: boolean;
}

export interface WorkspaceFileRepoTree {
  repoId: string;
  repoName: string;
  repoDisplayName: string;
  workspaceId: string;
  children: WorkspaceFileTreeNode[];
  truncated: boolean;
}
