import { createRoot } from 'react-dom/client';
import { KanbanTaskPanelContainer } from '@/pages/kanban/KanbanTaskPanelContainer';
import { TaskFloatingPanel } from '@/features/projects/ui/TaskFloatingPanel';
import {
  buildKanbanTaskComposerKey,
  openKanbanTaskComposer,
  useKanbanTaskComposerStore,
} from '@/shared/stores/useKanbanTaskComposerStore';
import { useUiPreferencesStore } from '@/shared/stores/useUiPreferencesStore';
import i18n from '@/i18n/config';
import { record, statuses, useFixtureState } from './mocks';

void i18n.changeLanguage('en');
const key = buildKanbanTaskComposerKey(null, 'project-1');
const legacy = new URLSearchParams(location.search).has('legacy');
openKanbanTaskComposer(key, { statusId: 'discuss' });
if (legacy) {
  // Simulate a previous release's saved preference AND restored composer draft.
  useUiPreferencesStore.setState(
    JSON.parse('{"createDraftWorkspaceByDefault":true}')
  );
  const entry = useKanbanTaskComposerStore.getState().byKey[key]!;
  useKanbanTaskComposerStore.setState({
    byKey: {
      [key]: {
        ...entry,
        draft: {
          ...entry.draft,
          title: 'Restored task',
          description: 'Restored description',
          ...JSON.parse('{"createDraftWorkspace":true}'),
        },
      },
    },
  });
}

function Fixture() {
  const { tasks, taskId } = useFixtureState();
  const composer = useKanbanTaskComposerStore((state) => state.byKey[key]);
  const task = tasks.find((task) => task.id === taskId);
  return (
    <>
      <output data-testid="composer-open">{String(Boolean(composer))}</output>
      {task ? (
        <aside aria-label="Task details">
          <TaskFloatingPanel
            task={task}
            statuses={statuses}
            tags={[]}
            selectedTagIds={new Set()}
            executions={[]}
            error={null}
            busyAction={null}
            agentUnavailableReason={null}
            workflowUnavailableReason={null}
            arenaUnavailableReason={null}
            relationships={null}
            comments={null}
            onClose={() => record('close')}
            onOpenExecution={() => record('open-execution')}
            getExecutionUnavailableReason={() => null}
            onCreateAgent={() => record('choose-agent')}
            onCreateWorkflow={() => record('choose-workflow')}
            onCreateArena={() => record('choose-arena')}
            onUpdateDescription={() => undefined}
            onUpdateStatus={() => undefined}
            onToggleTag={() => undefined}
          />
        </aside>
      ) : (
        <KanbanTaskPanelContainer
          taskResolution={null}
          onExpectTaskOpen={(id) => record('expect-task', id)}
        />
      )}
    </>
  );
}

createRoot(document.getElementById('root')!).render(<Fixture />);
