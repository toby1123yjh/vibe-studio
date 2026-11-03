/** Presentation state only. Canonical messages and drafts have their own owners. */
export interface WorkspaceConversationViewState {
  scrollTop: number;
  atBottom: boolean;
  anchor?: { key: string; offset: number };
}

/** Capture the visible message, not an offset based on transient row estimates. */
export function captureConversationReadingAnchor(element: HTMLElement) {
  const viewport = element.getBoundingClientRect();
  for (const row of element.querySelectorAll<HTMLElement>(
    '[data-semantic-key]'
  )) {
    const bounds = row.getBoundingClientRect();
    if (bounds.bottom > viewport.top && bounds.top < viewport.bottom) {
      const key = row.dataset.semanticKey;
      if (key) return { key, offset: bounds.top - viewport.top };
    }
  }
  return undefined;
}

export function conversationScopeKey(
  hostId: string | null,
  workspaceId: string,
  sessionId: string
) {
  return JSON.stringify([hostId, workspaceId, sessionId]);
}
