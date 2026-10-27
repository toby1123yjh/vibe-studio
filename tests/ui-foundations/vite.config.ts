import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { defineConfig, type Plugin } from 'vite';
import { getThemeBootstrapScript } from '../../packages/ui/src/lib/theme';

const repoRoot = fileURLToPath(new URL('../..', import.meta.url));
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

function themeBootstrapPlugin(): Plugin {
  return {
    name: 'theme-foundations-bootstrap',
    transformIndexHtml: {
      order: 'pre',
      handler() {
        return [
          {
            tag: 'script',
            children: getThemeBootstrapScript(),
            injectTo: 'head-prepend',
          },
        ];
      },
    },
  };
}

export default defineConfig({
  root: `${repoRoot}/tests/ui-foundations/fixture`,
  plugins: [themeBootstrapPlugin()],
  css: {
    postcss: {
      plugins: [
        tailwind({
          ...tailwindConfig,
          content: tailwindConfig.content.map((entry: string) =>
            resolve(repoRoot, 'packages/local-web', entry)
          ),
        }),
      ],
    },
  },
  resolve: {
    alias: {
      '@': `${repoRoot}/packages/web-core/src`,
      '@vibe/ui': `${repoRoot}/packages/ui/src`,
      ...Object.fromEntries(
        [
          '@lexical/react/LexicalComposer',
          '@lexical/react/LexicalComposerContext',
          '@lexical/react/LexicalRichTextPlugin',
          '@lexical/react/LexicalContentEditable',
          '@lexical/react/LexicalErrorBoundary',
          '@lexical/markdown',
          '@lexical/rich-text',
          'lexical',
        ].map((name) => [
          name,
          require.resolve(name, {
            paths: [resolve(repoRoot, 'packages/web-core')],
          }),
        ])
      ),
      shared: `${repoRoot}/shared`,
      '@tanstack/react-query': `${repoRoot}/packages/web-core/node_modules/@tanstack/react-query`,
      '@tanstack/react-router': `${repoRoot}/packages/web-core/node_modules/@tanstack/react-router`,
      react: `${repoRoot}/packages/ui/node_modules/react`,
      'react-dom': `${repoRoot}/packages/ui/node_modules/react-dom`,
      'lucide-react': `${repoRoot}/packages/ui/node_modules/lucide-react`,
    },
  },
  server: {
    fs: {
      allow: [repoRoot],
    },
  },
});
