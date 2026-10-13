import type { WorkflowNodeData, WorkflowNodeKind } from './workflowGraph';

export type WorkflowNodeFieldType =
  | 'text'
  | 'textarea'
  | 'select'
  | 'number'
  | 'condition_branches'
  | 'arena_attempts';

export interface WorkflowNodeFieldOption {
  label: string;
  value: string;
}

export interface WorkflowNodeFieldSchema {
  key: keyof WorkflowNodeData;
  type: WorkflowNodeFieldType;
  label: string;
  options?: WorkflowNodeFieldOption[];
  rows?: number;
}

export interface WorkflowNodeSchema {
  type: WorkflowNodeKind;
  label: string;
  fields: WorkflowNodeFieldSchema[];
}

export const WORKFLOW_NODE_SCHEMAS: Record<
  WorkflowNodeKind,
  WorkflowNodeSchema
> = {
  start: {
    type: 'start',
    label: 'Start',
    fields: [{ key: 'display_name', type: 'text', label: 'Title' }],
  },
  end: {
    type: 'end',
    label: 'End',
    fields: [{ key: 'display_name', type: 'text', label: 'Title' }],
  },
  agent: {
    type: 'agent',
    label: 'Agent',
    fields: [
      { key: 'display_name', type: 'text', label: 'Task title' },
      { key: 'role_template_id', type: 'text', label: 'Agent role' },
      {
        key: 'prompt_template',
        type: 'textarea',
        label: 'Task prompt',
        rows: 4,
      },
    ],
  },
  task_splitter: {
    type: 'task_splitter',
    label: 'Task Splitter',
    fields: [
      { key: 'display_name', type: 'text', label: 'Task title' },
      {
        key: 'prompt_template',
        type: 'textarea',
        label: 'Split instructions',
        rows: 5,
      },
    ],
  },
  condition: {
    type: 'condition',
    label: 'Condition',
    fields: [
      { key: 'display_name', type: 'text', label: 'Title' },
      {
        key: 'routing_mode',
        type: 'select',
        label: 'Routing Mode',
        options: [
          { label: 'Single branch', value: 'single' },
          { label: 'Multiple branches', value: 'multi' },
        ],
      },
      { key: 'branches', type: 'condition_branches', label: 'Branches' },
    ],
  },
  human_gate: {
    type: 'human_gate',
    label: 'Human Gate',
    fields: [
      { key: 'display_name', type: 'text', label: 'Title' },
      {
        key: 'prompt_to_human',
        type: 'textarea',
        label: 'Prompt to Human',
        rows: 3,
      },
      {
        key: 'required_action',
        type: 'select',
        label: 'Required Action',
        options: [
          { label: 'Approve Only', value: 'approve' },
          { label: 'Approve or Reject', value: 'approve_or_reject' },
        ],
      },
    ],
  },
  transform: {
    type: 'transform',
    label: 'Transform',
    fields: [
      { key: 'display_name', type: 'text', label: 'Title' },
      {
        key: 'mode',
        type: 'select',
        label: 'Mode',
        options: [
          { label: 'Template', value: 'template' },
          { label: 'Regex Extract', value: 'regex_extract' },
          { label: 'Truncate', value: 'truncate' },
        ],
      },
      { key: 'template', type: 'textarea', label: 'Template', rows: 3 },
      { key: 'regex', type: 'text', label: 'Regex' },
      { key: 'max_chars', type: 'number', label: 'Max Chars' },
    ],
  },
  arena: {
    type: 'arena',
    label: 'Arena',
    fields: [
      { key: 'display_name', type: 'text', label: 'Task title' },
      {
        key: 'prompt_template',
        type: 'textarea',
        label: 'Shared Task prompt',
        rows: 4,
      },
      { key: 'attempts', type: 'arena_attempts', label: 'Attempts' },
    ],
  },
};

export function getWorkflowNodeSchema(
  type: WorkflowNodeKind
): WorkflowNodeSchema {
  return WORKFLOW_NODE_SCHEMAS[type];
}

export function getWorkflowNodeSchemaFields(
  type: WorkflowNodeKind
): WorkflowNodeFieldSchema[] {
  return getWorkflowNodeSchema(type).fields;
}
