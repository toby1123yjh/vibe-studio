import { createFileRoute } from '@tanstack/react-router';
import { WorkflowLandingPage } from '@/features/workflow/ui/WorkflowLandingPage';
import { projectSearchValidator } from '@vibe/web-core/project-search';

export const Route = createFileRoute('/_app/workflows')({
  validateSearch: projectSearchValidator,
  component: WorkflowLandingPage,
});
