import { describe, expect, it } from 'vitest';
import type { WorkspaceFileContent } from 'shared/types';
import { classifyWorkspaceFile } from '@/shared/lib/fileClassification';

function content(
  overrides: Partial<WorkspaceFileContent>
): WorkspaceFileContent {
  return {
    workspace_id: 'workspace-1',
    repo_id: 'repo-1',
    repo_name: 'repo',
    path: 'src/file.ts',
    name: 'file.ts',
    kind: 'text',
    mime_type: 'text/plain',
    language: null,
    content: '',
    raw_url: null,
    size_bytes: 10n,
    truncated: false,
    ...overrides,
  };
}

describe('workspace file classification', () => {
  it('routes markdown files to the markdown viewer', () => {
    expect(
      classifyWorkspaceFile(
        content({
          path: 'README.md',
          name: 'README.md',
          mime_type: 'text/markdown',
        })
      )
    ).toMatchObject({
      viewKind: 'markdown',
      language: 'markdown',
      label: 'Markdown',
      isPreviewable: true,
    });
  });

  it('routes known source files to code with a language label', () => {
    expect(
      classifyWorkspaceFile(
        content({
          path: 'src/App.tsx',
          name: 'App.tsx',
          language: null,
        })
      )
    ).toMatchObject({
      viewKind: 'code',
      language: 'tsx',
      label: 'TSX',
      isPreviewable: true,
    });
  });

  it('treats HTML as source instead of executable preview content', () => {
    expect(
      classifyWorkspaceFile(
        content({
          path: 'public/index.html',
          name: 'index.html',
          mime_type: 'text/html',
        })
      )
    ).toMatchObject({
      viewKind: 'code',
      language: 'html',
      isPreviewable: true,
    });
  });

  it('routes safe image content to the image viewer', () => {
    expect(
      classifyWorkspaceFile(
        content({
          path: 'assets/logo.png',
          name: 'logo.png',
          kind: 'image',
          mime_type: 'image/png',
          raw_url: '/api/workspaces/workspace-1/files/raw?repo_id=repo-1',
        })
      )
    ).toMatchObject({
      viewKind: 'image',
      label: 'Image',
      isPreviewable: true,
    });
  });

  it('keeps binary files in the unsupported fallback', () => {
    expect(
      classifyWorkspaceFile(
        content({
          path: 'build/archive.zip',
          name: 'archive.zip',
          kind: 'binary',
          mime_type: 'application/zip',
          content: null,
        })
      )
    ).toMatchObject({
      viewKind: 'unsupported',
      isPreviewable: false,
    });
  });
});
