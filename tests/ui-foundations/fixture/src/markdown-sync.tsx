import * as React from 'react';
import { createRoot } from 'react-dom/client';
import { LexicalComposer } from '@lexical/react/LexicalComposer';
import { useLexicalComposerContext } from '@lexical/react/LexicalComposerContext';
import { RichTextPlugin } from '@lexical/react/LexicalRichTextPlugin';
import { ContentEditable } from '@lexical/react/LexicalContentEditable';
import { LexicalErrorBoundary } from '@lexical/react/LexicalErrorBoundary';
import {
  BOLD_STAR,
  HEADING,
  INLINE_CODE,
  type ElementTransformer,
} from '@lexical/markdown';
import { HeadingNode } from '@lexical/rich-text';
import { $getRoot } from 'lexical';
import { MarkdownSyncPlugin } from '@vibe/ui/components/MarkdownSyncPlugin';

const metrics = { exports: 0, changes: [] as string[], observed: 0 };
// An actual Markdown transformer counts export traversals; the editor and
// synchronization plugin are production components, not simulated updates.
const exportCounter: ElementTransformer = {
  type: 'element',
  dependencies: [],
  regExp: /^never-import-this-fixture-prefix:/,
  replace: () => {},
  export: () => {
    metrics.exports += 1;
    return null;
  },
};
const transformers = [exportCounter, HEADING, BOLD_STAR, INLINE_CODE];
const config = {
  namespace: 'markdown-sync-fixture',
  nodes: [HeadingNode],
  onError: (error: Error) => {
    throw error;
  },
};

export interface MarkdownSyncFixture {
  metrics: typeof metrics;
  replace: (value: string) => void;
  setEditable: (editable: boolean) => void;
  selectStart: () => void;
  reset: () => void;
}

declare global {
  interface Window {
    markdownSyncFixture: MarkdownSyncFixture;
  }
}

function Editor({ value, editable }: { value: string; editable: boolean }) {
  const [editor] = useLexicalComposerContext();
  React.useEffect(() => {
    window.markdownSyncFixture.selectStart = () => {
      editor.update(() => $getRoot().selectStart());
    };
  }, [editor]);

  return (
    <>
      <MarkdownSyncPlugin
        value={value}
        editable={editable}
        transformers={transformers}
        // WYSIWYGEditor supplies a callback even for read-only transcript rows.
        onChange={(markdown) => metrics.changes.push(markdown)}
        onEditorStateChange={() => {
          metrics.observed += 1;
        }}
      />
      <RichTextPlugin
        contentEditable={<ContentEditable aria-label="Markdown fixture" />}
        placeholder={null}
        ErrorBoundary={LexicalErrorBoundary}
      />
    </>
  );
}

function App() {
  const [value, replace] = React.useState('Initial **reply** with `file.ts`');
  const [editable, setEditable] = React.useState(
    new URLSearchParams(location.search).has('editable')
  );
  Object.assign(window, {
    markdownSyncFixture: {
      ...window.markdownSyncFixture,
      metrics,
      replace,
      setEditable,
      reset: () => {
        metrics.exports = 0;
        metrics.changes.length = 0;
        metrics.observed = 0;
      },
    },
  });
  return (
    <LexicalComposer initialConfig={config}>
      <Editor value={value} editable={editable} />
    </LexicalComposer>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
