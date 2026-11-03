import { memo } from 'react';
import WYSIWYGEditor from '@/shared/components/WYSIWYGEditor';
import { useChangesViewActions } from '@/shared/hooks/useChangesView';
import { ChatMarkdown } from '@vibe/ui/components/ChatMarkdown';
import { ReadOnlyChatMarkdown } from './ReadOnlyChatMarkdown';

/** Rich attachments and embedded review/component data retain their existing
 * renderers. Ordinary messages do not create editors or editor subscriptions. */
const RICH_MARKDOWN =
  /!\[|\]\((?:attachment:\/\/|pending-attachment:\/\/|\.vibe-attachments\/)|^\s*`{3,}(?:gh-comment|vk-component)\b/m;

export const AppChatMarkdown = memo(function AppChatMarkdown({
  content,
  workspaceId,
  sessionId,
  className,
  maxWidth,
}: {
  content: string;
  workspaceId?: string;
  sessionId?: string;
  className?: string;
  maxWidth?: string;
}) {
  const { viewFileInChanges, findMatchingDiffPath } = useChangesViewActions();
  return (
    <ChatMarkdown
      content={content}
      workspaceId={workspaceId}
      className={className}
      maxWidth={maxWidth}
      renderContent={({ content, className, workspaceId }) =>
        RICH_MARKDOWN.test(content) ? (
          <WYSIWYGEditor
            value={content}
            disabled
            className={className}
            workspaceId={workspaceId}
            sessionId={sessionId}
            findMatchingDiffPath={findMatchingDiffPath}
            onCodeClick={viewFileInChanges}
          />
        ) : (
          <ReadOnlyChatMarkdown
            content={content}
            className={className}
            findMatchingDiffPath={findMatchingDiffPath}
            onCodeClick={viewFileInChanges}
          />
        )
      }
    />
  );
});
