import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

const repoRoot = fileURLToPath(new URL('../..', import.meta.url));
const fixtureRoot = `${repoRoot}/tests/agent-center/fixture`;
const require = createRequire(import.meta.url);
const tailwind = require(
  `${repoRoot}/packages/local-web/node_modules/tailwindcss`
);
const loadConfig = require(
  `${repoRoot}/packages/local-web/node_modules/tailwindcss/loadConfig`
);
const tailwindConfig = loadConfig(
  `${repoRoot}/packages/local-web/tailwind.new.config.js`
);

export default defineConfig({
  root: fixtureRoot,
  plugins: [react()],
  css: {
    postcss: {
      plugins: [
        tailwind({
          ...tailwindConfig,
          content: [
            ...tailwindConfig.content.map((entry: string) =>
              resolve(repoRoot, 'packages/local-web', entry).replaceAll(
                '\\',
                '/'
              )
            ),
            `${fixtureRoot}/src/**/*.{ts,tsx}`,
          ],
        }),
      ],
    },
  },
  resolve: {
    alias: [
      {
        find: '@/features/agent-runtime/model/AgentRunSessionContext',
        replacement: `${fixtureRoot}/src/mocks.tsx`,
      },
      {
        find: /^@\/shared\/(hooks\/auth\/useAuth|providers\/HostIdProvider|lib\/(api|remoteApi|machineClient|relayPairingStorage))$/,
        replacement: `${fixtureRoot}/src/mocks.tsx`,
      },
      {
        find: /^@\/shared\/dialogs\/settings\/settings\/SettingsMachineUserSystemProvider$/,
        replacement: `${fixtureRoot}/src/mocks.tsx`,
      },
      {
        find: /^@\/shared\/hooks\/useUserSystem$/,
        replacement: `${fixtureRoot}/src/mocks.tsx`,
      },
      {
        find: /^@\/shared\/hooks\/useAppRuntime$/,
        replacement: `${fixtureRoot}/src/mocks.tsx`,
      },
      {
        find: /^@\/shared\/components\/AgentIcon$/,
        replacement: `${fixtureRoot}/src/mocks.tsx`,
      },
      {
        find: /^@\/shared\/hooks\/(useExecutorDiscovery|useSettingsNavigation)$/,
        replacement: `${fixtureRoot}/src/mocks.tsx`,
      },
      {
        find: '@tanstack/react-router',
        replacement: `${fixtureRoot}/src/mocks.tsx`,
      },
      {
        find: '@vibe/ui/components/StateSurface',
        replacement: `${fixtureRoot}/src/mocks.tsx`,
      },
      {
        find: '@vibe/ui/components/ConfirmDialog',
        replacement: `${fixtureRoot}/src/mocks.tsx`,
      },
      { find: '@', replacement: `${repoRoot}/packages/web-core/src` },
      { find: 'shared', replacement: `${repoRoot}/shared` },
      {
        find: 'react',
        replacement: `${repoRoot}/packages/web-core/node_modules/react`,
      },
      {
        find: 'react-dom',
        replacement: `${repoRoot}/packages/web-core/node_modules/react-dom`,
      },
      {
        find: '@tanstack/react-query',
        replacement: `${repoRoot}/packages/web-core/node_modules/@tanstack/react-query`,
      },
      {
        find: 'react-i18next',
        replacement: `${repoRoot}/packages/web-core/node_modules/react-i18next`,
      },
      {
        find: '@phosphor-icons/react',
        replacement: `${repoRoot}/packages/web-core/node_modules/@phosphor-icons/react`,
      },
      {
        find: 'react-hotkeys-hook',
        replacement: `${repoRoot}/packages/web-core/node_modules/react-hotkeys-hook`,
      },
      { find: '@vibe/ui', replacement: `${repoRoot}/packages/ui/src` },
    ],
  },
  server: { fs: { allow: [repoRoot] } },
});
