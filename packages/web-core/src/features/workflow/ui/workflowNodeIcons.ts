import {
  Bot,
  ListTree,
  GitBranch,
  Layers,
  Play,
  Square,
  User,
  Zap,
  type LucideIcon,
} from 'lucide-react';
import type { WorkflowNodeKind } from '../model/workflowGraph';

const NODE_ICONS: Record<WorkflowNodeKind, LucideIcon> = {
  start: Play,
  end: Square,
  agent: Bot,
  task_splitter: ListTree,
  condition: GitBranch,
  human_gate: User,
  transform: Layers,
  arena: Zap,
};

export function getWorkflowNodeIcon(kind: WorkflowNodeKind): LucideIcon {
  return NODE_ICONS[kind];
}
