import { useEffect, useRef } from 'react';
import { useLexicalComposerContext } from '@lexical/react/LexicalComposerContext';
import {
  $getSelection,
  $isRangeSelection,
  INDENT_CONTENT_COMMAND,
  KEY_TAB_COMMAND,
  KEY_MODIFIER_COMMAND,
  KEY_ENTER_COMMAND,
  OUTDENT_CONTENT_COMMAND,
  COMMAND_PRIORITY_NORMAL,
  COMMAND_PRIORITY_HIGH,
  type LexicalNode,
} from 'lexical';
import { $convertToMarkdownString, type Transformer } from '@lexical/markdown';
import { $isListItemNode } from '@lexical/list';
import { useTypeaheadOpen } from './TypeaheadOpenContext';

type SendMessageShortcut = 'ModifierEnter' | 'Enter';

type Props = {
  onCmdEnter?: (markdown: string) => void;
  onShiftCmdEnter?: (markdown: string) => void;
  transformers?: Transformer[];
  sendShortcut?: SendMessageShortcut;
};

export function KeyboardCommandsPlugin({
  onCmdEnter,
  onShiftCmdEnter,
  transformers,
  sendShortcut = 'Enter',
}: Props) {
  const [editor] = useLexicalComposerContext();
  const { isOpen: isTypeaheadOpen } = useTypeaheadOpen();
  const compositionRef = useRef({ active: false, endedAt: 0 });

  useEffect(() => {
    const onStart = () => {
      compositionRef.current.active = true;
    };
    const onEnd = () => {
      // Safari can deliver the candidate-confirming Enter after compositionend.
      compositionRef.current = { active: false, endedAt: Date.now() };
    };
    return editor.registerRootListener((root, previousRoot) => {
      previousRoot?.removeEventListener('compositionstart', onStart);
      previousRoot?.removeEventListener('compositionend', onEnd);
      compositionRef.current = { active: false, endedAt: 0 };
      root?.addEventListener('compositionstart', onStart);
      root?.addEventListener('compositionend', onEnd);
    });
  }, [editor]);

  useEffect(() => {
    const isComposing = (event: KeyboardEvent) =>
      event.isComposing ||
      event.keyCode === 229 ||
      editor.isComposing() ||
      compositionRef.current.active ||
      Date.now() - compositionRef.current.endedAt < 10;
    const isNodeInsideListItem = (node: LexicalNode): boolean => {
      if ($isListItemNode(node)) {
        return true;
      }
      return node.getParents().some($isListItemNode);
    };

    const isSelectionInsideListItem = (): boolean => {
      const selection = $getSelection();
      if (!$isRangeSelection(selection)) {
        return false;
      }

      return (
        isNodeInsideListItem(selection.anchor.getNode()) ||
        isNodeInsideListItem(selection.focus.getNode())
      );
    };

    const getSelectedListItem = (): LexicalNode | null => {
      const selection = $getSelection();
      if (!$isRangeSelection(selection)) {
        return null;
      }

      // On empty list items Lexical can include adjacent nodes in getNodes().
      // Prefer the last node so Tab applies to the cursor list item.
      const nodes = selection.getNodes();
      for (let i = nodes.length - 1; i >= 0; i--) {
        const node = nodes[i];
        if ($isListItemNode(node)) {
          return node;
        }
        const parentListItem = node.getParents().find($isListItemNode);
        if (parentListItem) {
          return parentListItem;
        }
      }

      const anchorNode = selection.anchor.getNode();
      if ($isListItemNode(anchorNode)) {
        return anchorNode;
      }
      return anchorNode.getParents().find($isListItemNode) ?? null;
    };

    const unregisterTab = editor.registerCommand(
      KEY_TAB_COMMAND,
      (event: KeyboardEvent) => {
        // Let typeahead use Tab for option selection.
        if (isTypeaheadOpen) {
          return false;
        }

        if (!isSelectionInsideListItem()) {
          return false;
        }

        event.preventDefault();
        const selection = $getSelection();
        if (!$isRangeSelection(selection)) {
          return false;
        }

        if (!selection.isCollapsed()) {
          return editor.dispatchCommand(
            event.shiftKey ? OUTDENT_CONTENT_COMMAND : INDENT_CONTENT_COMMAND,
            undefined
          );
        }

        const listItem = getSelectedListItem();
        if (!$isListItemNode(listItem)) {
          return false;
        }

        if (event.shiftKey) {
          const indent = listItem.getIndent();
          if (indent > 0) {
            listItem.setIndent(indent - 1);
          }
          return true;
        }

        // Match Google Docs behavior: first sibling cannot be indented further.
        if (!$isListItemNode(listItem.getPreviousSibling())) {
          return true;
        }

        listItem.setIndent(listItem.getIndent() + 1);
        return true;
      },
      COMMAND_PRIORITY_NORMAL
    );

    if (!onCmdEnter && !onShiftCmdEnter) {
      return unregisterTab;
    }

    // Commands already run inside the latest Lexical update. Reading that
    // state directly includes the last character without a React commit.
    const snapshot = () => $convertToMarkdownString(transformers);

    const unregisterModifier = editor.registerCommand(
      KEY_MODIFIER_COMMAND,
      (event: KeyboardEvent) => {
        if (!(event.metaKey || event.ctrlKey) || event.key !== 'Enter') {
          return false;
        }
        if (isComposing(event)) return true;
        if (isTypeaheadOpen) return false;
        if (event.altKey || (event.metaKey && event.ctrlKey)) return false;

        const submit = event.shiftKey
          ? onShiftCmdEnter
          : sendShortcut === 'ModifierEnter'
            ? onCmdEnter
            : undefined;
        if (!submit) return false;

        event.preventDefault();
        event.stopPropagation();
        if (!event.repeat) submit(snapshot());
        return true;
      },
      COMMAND_PRIORITY_NORMAL
    );

    const unregisterEnter = editor.registerCommand(
      KEY_ENTER_COMMAND,
      (event: KeyboardEvent | null) => {
        if (!event) return false;
        if (isComposing(event)) return true;

        // If typeahead is open, let it handle Enter
        if (isTypeaheadOpen) {
          return false;
        }

        if (sendShortcut === 'Enter') {
          if (
            event.shiftKey ||
            event.metaKey ||
            event.ctrlKey ||
            event.altKey
          ) {
            return false;
          }
          if (!onCmdEnter) return false;
          event.preventDefault();
          if (!event.repeat) onCmdEnter(snapshot());
          return true;
        }

        if (event.metaKey || event.ctrlKey) {
          return true;
        }

        return false;
      },
      COMMAND_PRIORITY_HIGH
    );

    return () => {
      unregisterTab();
      unregisterModifier();
      unregisterEnter();
    };
  }, [
    editor,
    onCmdEnter,
    onShiftCmdEnter,
    transformers,
    sendShortcut,
    isTypeaheadOpen,
  ]);

  return null;
}
