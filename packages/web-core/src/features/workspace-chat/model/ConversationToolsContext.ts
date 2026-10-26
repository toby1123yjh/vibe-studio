import { createContext, useContext } from 'react';
import type { RightMainPanelMode } from '@/shared/stores/useUiPreferencesStore';

export interface ConversationToolsActions {
  openTools: (mode?: RightMainPanelMode) => void;
  pendingComponentMarkdown: string | null;
  insertComponentContext: (markdown: string) => void;
  clearComponentContext: () => void;
}

export const ConversationToolsContext =
  createContext<ConversationToolsActions | null>(null);

export function useConversationToolsOptional() {
  return useContext(ConversationToolsContext);
}
