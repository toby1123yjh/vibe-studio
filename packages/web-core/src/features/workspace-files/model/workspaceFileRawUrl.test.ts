import { describe, expect, it } from 'vitest';
import { scopeWorkspaceFileRawUrl } from '@/shared/lib/workspaceFileRawUrl';

describe('workspace file raw urls', () => {
  it('leaves local raw urls unchanged without a host id', () => {
    expect(
      scopeWorkspaceFileRawUrl(
        '/api/workspaces/workspace-1/files/raw?repo_id=repo-1',
        null
      )
    ).toBe('/api/workspaces/workspace-1/files/raw?repo_id=repo-1');
  });

  it('scopes api raw urls for remote hosts', () => {
    expect(
      scopeWorkspaceFileRawUrl(
        '/api/workspaces/workspace-1/files/raw?repo_id=repo-1',
        'host-1'
      )
    ).toBe('/api/host/host-1/workspaces/workspace-1/files/raw?repo_id=repo-1');
  });

  it('does not double-scope host urls', () => {
    expect(
      scopeWorkspaceFileRawUrl(
        '/api/host/host-1/workspaces/workspace-1/files/raw',
        'host-1'
      )
    ).toBe('/api/host/host-1/workspaces/workspace-1/files/raw');
  });

  it('preserves external URLs', () => {
    expect(
      scopeWorkspaceFileRawUrl('https://example.com/file.png', 'host-1')
    ).toBe('https://example.com/file.png');
  });
});
