import { forwardRef, type RefObject } from 'react';
import {
  WorkspaceConversationContent,
  type WorkspaceConversationContentHandle,
  type WorkspaceConversationContentProps,
} from '@/features/workspace-chat/ui/WorkspaceConversationContent';
import { ContextBarContainer } from './ContextBarContainer';

export type WorkspacesMainContainerHandle = WorkspaceConversationContentHandle;

function renderContextBar(containerRef: RefObject<HTMLElement>) {
  return <ContextBarContainer containerRef={containerRef} />;
}

export const WorkspacesMainContainer = forwardRef<
  WorkspacesMainContainerHandle,
  WorkspaceConversationContentProps
>(function WorkspacesMainContainer(props, ref) {
  return (
    <WorkspaceConversationContent
      {...props}
      ref={ref}
      renderContextBar={renderContextBar}
    />
  );
});
