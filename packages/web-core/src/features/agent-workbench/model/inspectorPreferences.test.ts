import { expect, test } from '@playwright/test';
import {
  normalizeAgentWorkbenchInspectorPreferences,
  setAgentWorkbenchInspectorVisibility,
} from './inspectorPreferences';

const describe = test.describe;
const it = test;

describe('Agent Workbench Inspector preferences', () => {
  it('preserves tab and width while collapsed', () => {
    const state = normalizeAgentWorkbenchInspectorPreferences({
      activeTab: 'terminal',
      width: 440,
    });
    expect(setAgentWorkbenchInspectorVisibility(state, false)).toEqual({
      visible: false,
      activeTab: 'terminal',
      width: 440,
    });
  });

  it('clamps persisted width and rejects unknown tabs', () => {
    expect(
      normalizeAgentWorkbenchInspectorPreferences({
        activeTab: 'unknown' as 'files',
        width: 999,
      })
    ).toEqual({ visible: false, activeTab: 'files', width: 480 });
  });
});
