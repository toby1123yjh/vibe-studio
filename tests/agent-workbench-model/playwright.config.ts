import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '../../packages/web-core/src/features',
  testMatch: [
    '**/agent-workbench/model/*.test.ts',
    '**/agent-runtime/model/canonicalAgentConversation.test.ts',
    '**/agent-runtime/model/agentConversationStore.test.ts',
    '**/agent-runtime/model/streamingMarkdown.test.ts',
  ],
  fullyParallel: true,
  workers: 1,
  reporter: 'list',
});
