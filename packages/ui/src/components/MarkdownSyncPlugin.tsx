import { useEffect, useRef } from 'react';
import { useLexicalComposerContext } from '@lexical/react/LexicalComposerContext';
import {
  $convertToMarkdownString,
  $convertFromMarkdownString,
  type Transformer,
} from '@lexical/markdown';
import { $getRoot, type EditorState } from 'lexical';

type MarkdownSyncPluginProps = {
  value: string;
  onChange?: (markdown: string) => void;
  onEditorStateChange?: (state: EditorState) => void;
  editable: boolean;
  transformers: Transformer[];
  /** Initial Markdown already imported by LexicalComposer before first paint. */
  initialValue?: string;
};

const MARKDOWN_SYNC_TAG = 'markdown-sync';

/**
 * Handles bidirectional markdown synchronization between Lexical editor and external state.
 *
 * Uses an internal ref to prevent infinite update loops during bidirectional sync.
 */
export function MarkdownSyncPlugin({
  value,
  onChange,
  onEditorStateChange,
  editable,
  transformers,
  initialValue,
}: MarkdownSyncPluginProps) {
  const [editor] = useLexicalComposerContext();
  const lastSerializedRef = useRef<string | undefined>(initialValue);
  const prevTransformersRef = useRef(transformers);

  // Detect transformer changes and force re-parse
  if (transformers !== prevTransformersRef.current) {
    prevTransformersRef.current = transformers;
    lastSerializedRef.current = undefined;
  }

  // Handle editable state
  useEffect(() => {
    editor.setEditable(editable);
  }, [editor, editable]);

  // Handle controlled value changes (external → editor)
  useEffect(() => {
    if (value === lastSerializedRef.current) return;

    try {
      editor.update(
        () => {
          if (value.trim() === '') {
            $getRoot().clear();
          } else {
            $convertFromMarkdownString(value, transformers);
          }

          // Only position cursor at end if editor already has focus (user is actively editing)
          // This prevents unwanted focus when value changes externally (e.g., panel opening)
          const rootElement = editor.getRootElement();
          if (rootElement?.contains(document.activeElement)) {
            const root = $getRoot();
            const lastNode = root.getLastChild();
            if (lastNode) {
              lastNode.selectEnd();
            }
          }
        },
        { tag: MARKDOWN_SYNC_TAG }
      );
      lastSerializedRef.current = value;
    } catch (err) {
      console.error('Failed to parse markdown', err);
    }
  }, [editor, value, transformers]);

  // Handle editor changes (editor → external)
  useEffect(() => {
    // Transcript blocks only consume Markdown. Avoid a second full traversal
    // to serialize each incoming token (and avoid registering idle listeners).
    if ((!editable || !onChange) && !onEditorStateChange) return;

    return editor.registerUpdateListener(
      ({ editorState, dirtyElements, dirtyLeaves, tags }) => {
        onEditorStateChange?.(editorState);
        if (
          !editable ||
          !onChange ||
          tags.has(MARKDOWN_SYNC_TAG) ||
          (dirtyElements.size === 0 && dirtyLeaves.size === 0)
        ) {
          return;
        }

        const markdown = editorState.read(() =>
          $convertToMarkdownString(transformers)
        );

        if (markdown === lastSerializedRef.current) return;

        lastSerializedRef.current = markdown;
        onChange(markdown);
      }
    );
  }, [editor, editable, onChange, onEditorStateChange, transformers]);

  return null;
}
