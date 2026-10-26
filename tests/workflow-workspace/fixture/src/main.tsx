import React, { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import NiceModal from '@ebay/nice-modal-react';
import { HotkeysProvider } from 'react-hotkeys-hook';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useWorkflowRepositorySelection } from '@/features/workflow/ui/useWorkflowRepositorySelection';
import { workflowApi } from '@/shared/lib/workflowApi';
import { AppRuntimeProvider } from '@/shared/hooks/useAppRuntime';
import '@/i18n';
import '../../../../packages/ui/src/styles/tokens.css';

function Fixture() {
  const { selectWorkflowRepositories } = useWorkflowRepositorySelection({
    projectId: 'project-a',
    taskId: 'issue-a',
    taskTitle: 'Test',
  });
  const [result, setResult] = useState('');
  return (
    <>
      <button
        onClick={async () => {
          const workspace = await selectWorkflowRepositories();
          if (!workspace) {
            setResult('canceled');
            return;
          }
          await workflowApi.createAttempt('project-a', 'issue-a', {
            name: 'Test',
            graph_json: '{}',
            ...workspace,
          });
          await workflowApi.runAttempt('attempt-a', {
            workspace_id: null,
            trigger_source: 'manual',
            input_text: 'Test',
            ...workspace,
          });
          setResult(JSON.stringify(workspace));
        }}
      >
        Choose workflow location
      </button>
      <output>{result}</output>
    </>
  );
}
function RouteFixture() {
  const [mounted, setMounted] = useState(true);
  useEffect(() => {
    const unmount = () => setMounted(false);
    window.addEventListener('unmount-workflow-fixture', unmount);
    return () =>
      window.removeEventListener('unmount-workflow-fixture', unmount);
  }, []);
  return mounted ? <Fixture /> : <p>Workflow route closed</p>;
}

createRoot(document.getElementById('root')!).render(
  <QueryClientProvider client={new QueryClient()}>
    <HotkeysProvider>
      <AppRuntimeProvider runtime="local">
        <NiceModal.Provider>
          <RouteFixture />
        </NiceModal.Provider>
      </AppRuntimeProvider>
    </HotkeysProvider>
  </QueryClientProvider>
);
