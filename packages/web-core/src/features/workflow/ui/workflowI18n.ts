import type { TFunction } from 'i18next';
import type {
  WorkflowGraphDefaultLabels,
  WorkflowNodeData,
  WorkflowNodeKind,
} from '../model/workflowGraph';
import type { WorkflowAuthoringIssue } from '../model/workflowAuthoring';

export function getWorkflowDefaultGraphLabels(
  t: TFunction<'common'>
): WorkflowGraphDefaultLabels {
  return {
    startLabel: t('workflow.nodes.start.label'),
    endLabel: t('workflow.nodes.end.label'),
    familiarizeLabel: t('workflow.defaultGraph.familiarizeLabel'),
    agentPrompt: t('workflow.defaultGraph.agentPrompt'),
    stageTitle: t('workflow.defaultGraph.stageTitle'),
    stageDescription: t('workflow.defaultGraph.stageDescription'),
  };
}

export function getWorkflowDefaultNodeData(
  kind: WorkflowNodeKind,
  t: TFunction<'common'>
): Partial<WorkflowNodeData> {
  switch (kind) {
    case 'start':
      return { display_name: t('workflow.nodes.start.label') };
    case 'end':
      return { display_name: t('workflow.nodes.end.label') };
    case 'agent':
      return { display_name: t('workflow.nodes.agent.label') };
    case 'task_splitter':
      return {
        display_name: t('workflow.nodes.taskSplitter.label'),
        prompt_template: t('workflow.split.defaultPrompt'),
      };
    case 'condition':
      return { display_name: t('workflow.nodes.condition.label') };
    case 'human_gate':
      return { display_name: t('workflow.nodes.humanGate.label') };
    case 'transform':
      return { display_name: t('workflow.nodes.transform.label') };
    case 'arena':
      return {
        display_name: t('workflow.nodes.arena.label'),
        attempts: [
          {
            id: 'attempt-a',
            display_name: t('workflow.nodes.arena.attemptA'),
            role_template_id: 'custom',
            prompt_template: '',
          },
          {
            id: 'attempt-b',
            display_name: t('workflow.nodes.arena.attemptB'),
            role_template_id: 'custom',
            prompt_template: '',
          },
        ],
      };
  }
}

export function getWorkflowAuthoringIssueMessage(
  issue: WorkflowAuthoringIssue,
  t: TFunction<'common'>
): string {
  if (issue.code === 'splitter-scope' && issue.splitScopeCode) {
    return t(`workflow.split.scopeErrors.${issue.splitScopeCode}`);
  }
  const keyByCode: Partial<Record<WorkflowAuthoringIssue['code'], string>> = {
    'missing-node': 'missingNode',
    'self-connection': 'selfConnection',
    'end-source': 'endSource',
    'start-target': 'startTarget',
    'invalid-source-handle': 'invalidSourceHandle',
    'occupied-source-handle': 'occupiedSourceHandle',
    'duplicate-connection': 'duplicateConnection',
    'unconnected-branch': 'unconnectedBranch',
    'too-few-candidates': 'tooFewCandidates',
  };
  const requiredFieldKey: Record<string, string> = {
    display_name: 'nodeTitleRequired',
    prompt_template: 'taskPromptRequired',
    executor_config: 'agentRequired',
    branches: 'branchesRequired',
    prompt_to_human: 'approvalRequestRequired',
    template: 'transformTemplateRequired',
    regex: 'transformRegexRequired',
    max_chars: 'transformMaxCharsRequired',
  };
  const messageKey =
    issue.code === 'required-field'
      ? requiredFieldKey[issue.field ?? '']
      : keyByCode[issue.code];

  return messageKey
    ? t(`workflow.authoringErrors.${messageKey}`, {
        defaultValue: issue.message,
      })
    : issue.message;
}

export function workflowNodeStatusKey(status: string): string {
  switch (status) {
    case 'awaiting_human':
      return 'awaitingHuman';
    case 'awaiting_arena':
      return 'awaitingArena';
    default:
      return status;
  }
}

export function workflowSplitStateKey(state: string): string {
  const states = new Set([
    'pending',
    'accepted',
    'dispatching',
    'running',
    'partial',
    'succeeded',
    'failed',
    'cancelled',
    'queued',
    'blocked',
    'claimed',
    'completed',
  ]);
  return states.has(state) ? state : 'unknown';
}
