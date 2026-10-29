import { useState } from 'react';
import { useCreateWorkspace } from '@/shared/hooks/useCreateWorkspace';
import { WorkspaceSessionConversation } from '@/features/workspace-chat/ui/WorkspaceSessionConversation';

/** Real creation mutation and destination; tests provide only API transport. */
export function WorkspaceCreationHandoffPreview() {
  const { createWorkspace } = useCreateWorkspace();
  const [binding, setBinding] = useState<{
    workspaceId: string;
    sessionId: string;
  } | null>(null);

  return (
    <main className="new-design flex h-screen flex-col">
      {binding ? (
        <WorkspaceSessionConversation {...binding} />
      ) : (
        <button
          disabled={createWorkspace.isPending}
          onClick={async () => {
            const result = await createWorkspace.mutateAsync({
              data: {
                mode: 'managed_directory',
                executor_config: { executor: 'CODEX' },
                prompt: 'Create a new conversation',
                repos: [],
              },
            });
            if (result.workspace)
              setBinding({
                workspaceId: result.workspace.id,
                sessionId: result.agentRun.state.session_id,
              });
          }}
        >
          {createWorkspace.isPending
            ? 'Starting conversation…'
            : 'Create conversation'}
        </button>
      )}
    </main>
  );
}
