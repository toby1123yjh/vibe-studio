import { expect, test } from '@playwright/test';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import {
  buildWorkspaceContext,
  joinWorkspacePath,
  resolveWorkspaceWorkingDirectory,
} from './workspaceContext';

test.describe('shared filesystem path display', () => {
  const cases = [
    ['', ''],
    ['C:\\', 'C:\\'],
    ['\\\\?\\C:\\', 'C:\\'],
    ['\\\\?\\d:\\目录 with spaces\\文件.txt', 'd:\\目录 with spaces\\文件.txt'],
    ['\\\\?\\UNC\\host\\share\\目录', '\\\\host\\share\\目录'],
    ['\\\\?\\uNc\\host\\share\\', '\\\\host\\share\\'],
    ['\\\\host\\share', '\\\\host\\share'],
    ['/home/user/目录', '/home/user/目录'],
    ['../relative/file.txt', '../relative/file.txt'],
    ['C:relative\\file.txt', 'C:relative\\file.txt'],
    ['\\\\.\\pipe\\agent', '\\\\.\\pipe\\agent'],
    ['\\\\?\\Volume{abc}\\folder', '\\\\?\\Volume{abc}\\folder'],
    [
      '\\\\?\\GLOBALROOT\\Device\\HarddiskVolume1',
      '\\\\?\\GLOBALROOT\\Device\\HarddiskVolume1',
    ],
    ['/tmp/\\\\?\\C:\\literal', '/tmp/\\\\?\\C:\\literal'],
    ['https://host/path', 'https://host/path'],
  ];

  for (const [nativePath, displayPath] of cases) {
    test('preserves path meaning: ' + JSON.stringify(nativePath), () => {
      expect(formatPathForDisplay(nativePath)).toBe(displayPath);
      expect(formatPathForDisplay(displayPath)).toBe(displayPath);
    });
  }

  test('does not shorten long paths or modify their contents', () => {
    const suffix = 'directory with spaces\\'.repeat(20) + '文件.txt';
    expect(formatPathForDisplay('\\\\?\\D:\\' + suffix)).toBe('D:\\' + suffix);
  });
});

test.describe('workspace context projection', () => {
  test('joins Windows and Unix workspace paths', () => {
    expect(joinWorkspacePath('F:\\workspaces', 'repo')).toBe(
      'F:\\workspaces\\repo'
    );
    expect(joinWorkspacePath('/tmp/workspaces/', '/repo')).toBe(
      '/tmp/workspaces/repo'
    );
  });

  test('resolves the exact agent working directory without Vibe history state', () => {
    expect(
      resolveWorkspaceWorkingDirectory({
        containerRef: 'F:\\workspaces',
        workingDir: 'repo',
        fallbackRepoName: 'ignored',
      })
    ).toBe('F:\\workspaces\\repo');

    expect(
      resolveWorkspaceWorkingDirectory({
        containerRef: 'F:\\source-repo',
      })
    ).toBe('F:\\source-repo');

    expect(
      resolveWorkspaceWorkingDirectory({
        containerRef: '   ',
      })
    ).toBeUndefined();
  });

  test('shows a Git direct folder without a mode label', () => {
    expect(
      buildWorkspaceContext({
        containerRef: 'F:\\repo',
        branch: 'main',
        workspaceKind: 'direct_folder',
      })
    ).toEqual([
      { kind: 'path', label: 'F:\\repo' },
      { kind: 'branch', label: 'main' },
    ]);
  });

  test('hides the non-Git direct-folder persistence sentinel', () => {
    expect(
      buildWorkspaceContext({
        containerRef: 'F:\\notes',
        branch: 'direct-folder',
        workspaceKind: 'direct_folder',
      })
    ).toEqual([{ kind: 'path', label: 'F:\\notes' }]);
  });

  test('shows the actual worktree path, branch, and marker', () => {
    expect(
      buildWorkspaceContext({
        containerRef: 'F:\\workspaces',
        workingDir: 'repo',
        branch: 'feature/workflow',
        workspaceKind: 'worktree',
      })
    ).toEqual([
      { kind: 'path', label: 'F:\\workspaces\\repo' },
      { kind: 'branch', label: 'feature/workflow' },
      { kind: 'mode', label: 'Worktree' },
    ]);
  });

  for (const { path, displayPath } of [
    {
      path: '\\\\?\\F:\\Mydev2023\\serverSpace',
      displayPath: 'F:\\Mydev2023\\serverSpace',
    },
    {
      path: '\\\\?\\UNC\\server\\share',
      displayPath: '\\\\server\\share',
    },
    { path: '\\\\?\\unc\\server\\share', displayPath: '\\\\server\\share' },
    { path: '\\\\server\\share', displayPath: '\\\\server\\share' },
    { path: 'C:\\projects', displayPath: 'C:\\projects' },
    { path: '/home/user/projects', displayPath: '/home/user/projects' },
    { path: '\\\\?\\Volume{abc}\\', displayPath: '\\\\?\\Volume{abc}\\' },
  ]) {
    test(`formats ${path} for display without changing the operational path`, () => {
      const context = { containerRef: path };
      expect(buildWorkspaceContext(context)).toEqual([
        { kind: 'path', label: displayPath },
      ]);
      expect(resolveWorkspaceWorkingDirectory(context)).toBe(path);
    });
  }

  test('formats the complete agent subdirectory and preserves branch and mode', () => {
    const context = {
      containerRef: '\\\\?\\UNC\\server\\share',
      workingDir: 'repo',
      branch: 'feature/workflow',
      workspaceKind: 'worktree' as const,
    };
    expect(buildWorkspaceContext(context)).toEqual([
      { kind: 'path', label: '\\\\server\\share\\repo' },
      { kind: 'branch', label: 'feature/workflow' },
      { kind: 'mode', label: 'Worktree' },
    ]);
    expect(resolveWorkspaceWorkingDirectory(context)).toBe(
      '\\\\?\\UNC\\server\\share\\repo'
    );
  });
});
