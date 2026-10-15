/** Presentation state only. Canonical messages and drafts have their own owners. */
export interface WorkspaceConversationViewState {
  scrollTop: number;
  atBottom: boolean;
}

export function conversationScopeKey(
  hostId: string | null,
  workspaceId: string,
  sessionId: string
) {
  return JSON.stringify([hostId, workspaceId, sessionId]);
}
