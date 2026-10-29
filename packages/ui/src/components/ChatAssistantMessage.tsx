import type { ReactNode } from 'react';

export interface ChatAssistantMessageRenderProps {
  content: string;
  workspaceId?: string;
}

interface ChatAssistantMessageProps {
  content: string;
  workspaceId?: string;
  renderMarkdown: (props: ChatAssistantMessageRenderProps) => ReactNode;
}

export function ChatAssistantMessage({
  content,
  workspaceId,
  renderMarkdown,
}: ChatAssistantMessageProps) {
  return (
    <div
      className="min-w-0 text-normal leading-relaxed [overflow-wrap:anywhere] [&_img]:max-w-full [&_pre]:max-w-full [&_pre]:overflow-x-auto [&_table]:block [&_table]:max-w-full [&_table]:overflow-x-auto"
      data-chat-message="assistant"
    >
      {renderMarkdown({ content, workspaceId })}
    </div>
  );
}
