import { expect, test } from '@playwright/test';
import {
  SHELL_MODULE_ORDER,
  activateShellModuleCapability,
  appShellDiscoveryQueryKey,
  createAppShellDiscoveryScopeKey,
  deriveActiveShellModule,
  derivePageCanvasMode,
  deriveSidebarSectionViewState,
  mergeStableCursorItems,
  type AppShellModuleCapability,
} from './appShell';

test.describe('App Shell model', () => {
  test('keeps the approved primary navigation order with Search as a trigger', () => {
    expect(SHELL_MODULE_ORDER).toEqual([
      'dashboard',
      'search',
      'projects',
      'workflows',
      'agents',
    ]);
    expect(deriveActiveShellModule('/search')).toBeNull();
  });

  test('requires a reason for unavailable modules and never activates them', () => {
    let navigationCount = 0;
    const unavailable = {
      availability: 'unavailable',
      reason: 'The owning host is offline.',
    } satisfies AppShellModuleCapability;
    const available = {
      availability: 'available',
      navigate: () => {
        navigationCount += 1;
      },
    } satisfies AppShellModuleCapability;

    expect(activateShellModuleCapability(unavailable)).toBe(false);
    expect(navigationCount).toBe(0);
    expect(activateShellModuleCapability(available)).toBe(true);
    expect(navigationCount).toBe(1);
  });

  test('derives stable deep-route active modules', () => {
    expect(deriveActiveShellModule('/projects/p-1/tasks/i-1')).toBeNull();
    expect(deriveActiveShellModule('/projects/p-1/workflows')).toBe(
      'workflows'
    );
    expect(deriveActiveShellModule('/projects/p-1/workflow-runs/run-1')).toBe(
      'workflows'
    );
    expect(deriveActiveShellModule('/agents/codex')).toBe('agents');
  });

  test('highlights Projects only on its directory, not individual projects', () => {
    expect(deriveActiveShellModule('/projects')).toBe('projects');
    expect(deriveActiveShellModule('/projects/')).toBe('projects');
    expect(deriveActiveShellModule('/projects/p-1')).toBeNull();
    expect(deriveActiveShellModule('/projects/p-2/')).toBeNull();
  });

  test('deduplicates cursor pages and preserves canonical ordering', () => {
    const result = mergeStableCursorItems(
      [
        { id: 'b', updated_at: '2026-08-29T10:00:00Z' },
        { id: 'a', updated_at: '2026-08-29T10:00:00Z' },
      ],
      [
        { id: 'b', updated_at: '2026-08-29T10:00:00Z' },
        { id: 'c', updated_at: '2026-08-28T10:00:00Z' },
      ]
    );
    expect(result.map((item) => item.id)).toEqual(['a', 'b', 'c']);
  });

  test('does not let selection participate in ordering', () => {
    const source = [
      { id: 'older', updated_at: '2026-08-28T10:00:00Z', active: true },
      { id: 'newer', updated_at: '2026-08-29T10:00:00Z', active: false },
    ];
    expect(mergeStableCursorItems([], source).map((item) => item.id)).toEqual([
      'newer',
      'older',
    ]);
  });

  test('uses contained templates only for module landing pages', () => {
    expect(derivePageCanvasMode('/dashboard')).toBe('full-bleed');
    expect(derivePageCanvasMode('/projects/')).toBe('contained');
    expect(derivePageCanvasMode('/agents/codex')).toBe('contained');
    expect(derivePageCanvasMode('/projects/p-1')).toBe('full-bleed');
    expect(derivePageCanvasMode('/workspaces/w-1')).toBe('full-bleed');
  });

  test('scopes discovery caches by deployment, Host, and user identity', () => {
    const localUserOne = createAppShellDiscoveryScopeKey({
      deployment: 'local',
      hostId: 'host-1',
      userId: 'user-1',
    });
    const localUserTwo = createAppShellDiscoveryScopeKey({
      deployment: 'local',
      hostId: 'host-1',
      userId: 'user-2',
    });
    const remoteUserOne = createAppShellDiscoveryScopeKey({
      deployment: 'remote',
      hostId: 'host-1',
      userId: 'user-1',
    });

    expect(localUserOne).not.toBe(localUserTwo);
    expect(localUserOne).not.toBe(remoteUserOne);
    expect(appShellDiscoveryQueryKey(localUserOne, 'projects')).not.toEqual(
      appShellDiscoveryQueryKey(localUserOne, 'sessions')
    );
    expect(appShellDiscoveryQueryKey(localUserOne, 'projects')).not.toEqual(
      appShellDiscoveryQueryKey(localUserTwo, 'projects')
    );
  });

  test('distinguishes first load, empty, initial error, and cached degradation', () => {
    expect(
      deriveSidebarSectionViewState({
        itemCount: 0,
        isLoading: true,
        isError: false,
      })
    ).toBe('loading');
    expect(
      deriveSidebarSectionViewState({
        itemCount: 0,
        isLoading: false,
        isError: false,
      })
    ).toBe('empty');
    expect(
      deriveSidebarSectionViewState({
        itemCount: 0,
        isLoading: false,
        isError: true,
      })
    ).toBe('error');
    expect(
      deriveSidebarSectionViewState({
        itemCount: 2,
        isLoading: false,
        isError: true,
      })
    ).toBe('degraded');
    expect(
      deriveSidebarSectionViewState({
        itemCount: 2,
        isLoading: false,
        isError: false,
      })
    ).toBe('ready');
  });
});
