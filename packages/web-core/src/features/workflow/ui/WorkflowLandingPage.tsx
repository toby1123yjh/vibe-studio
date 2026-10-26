import { useHostId } from '@/shared/providers/HostIdProvider';
import { WorkflowTemplateListPage } from './WorkflowTemplateListPage';

/** Workflow discovery is independent of the project's sidebar selection. */
export function WorkflowLandingPage() {
  const hostId = useHostId();
  return <WorkflowTemplateListPage key={hostId ?? 'local'} />;
}
