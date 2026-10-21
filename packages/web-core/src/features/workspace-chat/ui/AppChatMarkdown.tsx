import WYSIWYGEditor from '@/shared/components/WYSIWYGEditor';
import { useChangesViewActions } from '@/shared/hooks/useChangesView';
import { ChatMarkdown } from '@vibe/ui/components/ChatMarkdown';

/** The existing chat Markdown renderer, shared by canonical blocks and script
 * entries. Keep file/code interactions on the same application boundary. */
export function AppChatMarkdown({
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
      renderContent={({ content, className, workspaceId }) => (
        <WYSIWYGEditor
          value={content}
          disabled
          className={className}
          workspaceId={workspaceId}
          sessionId={sessionId}
          findMatchingDiffPath={findMatchingDiffPath}
          onCodeClick={viewFileInChanges}
        />
      )}
    />
  );
}
