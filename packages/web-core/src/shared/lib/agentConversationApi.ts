import type {
  ConversationBlockDetail,
  ConversationChangesPage,
  ConversationCursor,
  ConversationPage,
} from 'shared/types';
import { handleApiResponse } from '@/shared/lib/api';
import {
  makeLocalApiRequest,
  openLocalApiWebSocket,
} from '@/shared/lib/localApiTransport';

function conversationPath(sessionId: string) {
  return `/api/sessions/${encodeURIComponent(sessionId)}/conversation`;
}

export const agentConversationApi = {
  async page(
    sessionId: string,
    hostId: string | null,
    before?: string | null,
    signal?: AbortSignal
  ): Promise<ConversationPage> {
    const query = new URLSearchParams({ limit: '50' });
    if (before) query.set('before', before);
    return handleApiResponse<ConversationPage>(
      await makeLocalApiRequest(`${conversationPath(sessionId)}?${query}`, {
        hostScope: 'explicit',
        hostId,
        signal,
      })
    );
  },

  async changes(
    sessionId: string,
    hostId: string | null,
    cursor: ConversationCursor,
    signal?: AbortSignal
  ): Promise<ConversationChangesPage> {
    const query = new URLSearchParams({
      epoch: cursor.epoch,
      after_revision: cursor.revision,
      limit: '50',
    });
    return handleApiResponse<ConversationChangesPage>(
      await makeLocalApiRequest(
        `${conversationPath(sessionId)}/changes?${query}`,
        { hostScope: 'explicit', hostId, signal }
      )
    );
  },

  async detail(
    sessionId: string,
    hostId: string | null,
    blockId: string,
    field: string,
    offset = 0,
    signal?: AbortSignal
  ): Promise<ConversationBlockDetail> {
    const query = new URLSearchParams({
      field,
      offset: String(offset),
      limit: '32768',
    });
    return handleApiResponse<ConversationBlockDetail>(
      await makeLocalApiRequest(
        `${conversationPath(sessionId)}/blocks/${encodeURIComponent(blockId)}?${query}`,
        { hostScope: 'explicit', hostId, signal }
      )
    );
  },

  openStream(
    sessionId: string,
    hostId: string | null,
    cursor: ConversationCursor
  ): Promise<WebSocket> {
    const query = new URLSearchParams({
      epoch: cursor.epoch,
      after_revision: cursor.revision,
    });
    return openLocalApiWebSocket(
      `${conversationPath(sessionId)}/stream/ws?${query}`,
      { hostScope: 'explicit', hostId }
    );
  },
};
