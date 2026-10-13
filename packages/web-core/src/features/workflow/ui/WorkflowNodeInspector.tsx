import type {
  WorkflowConditionBranch,
  WorkflowGraph,
  WorkflowNode,
  WorkflowNodeData,
  WorkflowNodeKind,
} from '../model/workflowGraph';
import type { TFunction } from 'i18next';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  getWorkflowNodeSchema,
  type WorkflowNodeFieldSchema,
} from '../model/workflowNodeSchemas';
import { getWorkflowAgentDisplay } from '../model/workflowAgentDisplay';
import { AgentIcon } from '@/shared/components/AgentIcon';
import { getWorkflowSplitScopes } from '../model/workflowSplitScope';
import { WorkflowNodeFieldRenderer } from './WorkflowNodeFieldRenderer';
import { WorkflowAgentExecutorField } from './WorkflowAgentExecutorField';
import { Plus, Settings2, Trash2 } from 'lucide-react';
import {
  applyWorkflowTransform,
  type WorkflowTransformResult,
} from '../model/workflowAuthoring';

export interface WorkflowNodeInspectorProps {
  node: WorkflowNode | null;
  graph?: WorkflowGraph | null;
  routerExecutorConfig?: unknown;
  readOnly?: boolean;
  onChange?: (nodeId: string, data: Partial<WorkflowNodeData>) => void;
  onConfigureRouter?: () => void;
  onDelete?: (nodeId: string) => void;
}

const simpleFieldTypes = new Set(['text', 'textarea', 'select', 'number']);

function isSimpleField(field: WorkflowNodeFieldSchema) {
  return simpleFieldTypes.has(field.type);
}

function shouldRenderSimpleField(
  type: WorkflowNodeKind,
  data: WorkflowNodeData,
  field: WorkflowNodeFieldSchema
) {
  if (type !== 'transform') {
    return true;
  }

  if (field.key === 'template') {
    return (data.mode ?? 'template') === 'template';
  }
  if (field.key === 'regex') {
    return data.mode === 'regex_extract';
  }
  if (field.key === 'max_chars') {
    return data.mode === 'truncate';
  }
  return true;
}

function getLocalizedField(
  field: WorkflowNodeFieldSchema,
  t: TFunction<'common'>
): WorkflowNodeFieldSchema {
  const label = t(`workflow.fields.${String(field.key)}`, {
    defaultValue: field.label,
  });
  const options = field.options?.map((option) => ({
    ...option,
    label: t(`workflow.fieldOptions.${String(field.key)}.${option.value}`, {
      defaultValue: option.label,
    }),
  }));
  return { ...field, label, ...(options ? { options } : {}) };
}

export function WorkflowNodeInspector({
  node,
  graph,
  routerExecutorConfig,
  readOnly,
  onChange,
  onConfigureRouter,
  onDelete,
}: WorkflowNodeInspectorProps) {
  const { t } = useTranslation('common');
  const [transformTests, setTransformTests] = useState<
    Record<
      string,
      { open: boolean; input: string; result: WorkflowTransformResult | null }
    >
  >({});
  if (!node) {
    return (
      <div className="flex h-full items-center justify-center p-base text-center text-low text-sm">
        {t('workflow.inspector.selectNode')}
      </div>
    );
  }

  const handleChange = (key: keyof WorkflowNodeData, value: unknown) => {
    if (readOnly || !onChange) return;
    onChange(node.id, { [key]: value });
  };

  const { data, type } = node;
  const schema = getWorkflowNodeSchema(type);
  const schemaFields = schema.fields;
  const simpleFields = schemaFields
    .filter(isSimpleField)
    .filter((field) => shouldRenderSimpleField(type, data, field))
    .map((field) =>
      type === 'task_splitter' && field.key === 'prompt_template'
        ? { ...field, label: t('workflow.split.instructions') }
        : getLocalizedField(field, t)
    );
  const splitScopes = graph ? getWorkflowSplitScopes(graph) : null;
  const splitScope = splitScopes?.scopes.find(
    (scope) => scope.splitterNodeId === node.id
  );
  const splitScopeIssue = splitScopes?.issues.find(
    (issue) => issue.nodeId === node.id
  );
  const hasConditionBranches = schemaFields.some(
    (field) => field.type === 'condition_branches'
  );
  const hasArenaAttempts = schemaFields.some(
    (field) => field.type === 'arena_attempts'
  );
  const branches = data.branches ?? [];
  const attempts = data.attempts ?? [];
  const routerDisplay = getWorkflowAgentDisplay({
    executor_config: routerExecutorConfig,
  });
  const hasRouterExecutorConfig = routerDisplay.executorConfig !== null;
  const transformTest = transformTests[node.id] ?? {
    open: false,
    input: '',
    result: null,
  };
  const updateTransformTest = (
    updates: Partial<(typeof transformTests)[string]>
  ) => {
    setTransformTests((current) => ({
      ...current,
      [node.id]: {
        ...(current[node.id] ?? transformTest),
        ...updates,
      },
    }));
  };

  const inputClass =
    'w-full rounded border border-secondary bg-primary px-3 py-1.5 text-sm transition-colors focus:border-brand focus:outline-none focus:ring-1 focus:ring-brand disabled:opacity-50';
  const secondaryButtonClass =
    'inline-flex h-8 items-center justify-center gap-1.5 rounded border border-secondary bg-primary px-2.5 text-xs font-medium text-high transition-colors hover:border-brand hover:text-brand disabled:cursor-not-allowed disabled:opacity-50';
  const iconButtonClass =
    'inline-flex h-8 w-8 items-center justify-center rounded border border-secondary bg-primary text-low transition-colors hover:border-error/70 hover:text-error disabled:cursor-not-allowed disabled:opacity-50';

  return (
    <div className="workflow-inspector-body flex min-h-full flex-col gap-4 p-4 text-sm">
      {simpleFields.map((field) => (
        <WorkflowNodeFieldRenderer
          key={String(field.key)}
          data={data}
          field={field}
          inputClassName={inputClass}
          readOnly={readOnly}
          onChange={handleChange}
        />
      ))}

      {type === 'agent' || type === 'task_splitter' ? (
        <WorkflowAgentExecutorField
          key={node.id}
          value={data.executor_config}
          readOnly={readOnly}
          onChange={(executorConfig) =>
            handleChange('executor_config', executorConfig)
          }
        />
      ) : null}

      {type === 'task_splitter' ? (
        <section className="space-y-3 border-t border-secondary/60 pt-4">
          <h3 className="text-xs font-semibold text-high">
            {t('workflow.split.fixedScope')}
          </h3>
          <p className="text-xs leading-relaxed text-low">
            {t('workflow.split.scopeHint')}
          </p>
          {splitScope ? (
            <dl className="space-y-2 text-xs">
              <div>
                <dt className="text-low">{t('workflow.split.executors')}</dt>
                <dd className="mt-1 space-y-1 text-high">
                  {splitScope.candidateNodeIds.map((id) => (
                    <p key={id}>
                      {graph?.nodes.find((candidate) => candidate.id === id)
                        ?.data.display_name || id}
                    </p>
                  ))}
                </dd>
              </div>
              <div>
                <dt className="text-low">{t('workflow.split.summaryExit')}</dt>
                <dd className="mt-1 text-high">
                  {graph?.nodes.find(
                    (candidate) => candidate.id === splitScope.summaryNodeId
                  )?.data.display_name || splitScope.summaryNodeId}
                </dd>
              </div>
            </dl>
          ) : (
            <p className="text-xs text-warning" role="status">
              {splitScopeIssue
                ? t(`workflow.split.scopeErrors.${splitScopeIssue.code}`)
                : t('workflow.split.scopeErrors.missingExecutors')}
            </p>
          )}
        </section>
      ) : null}

      {type === 'condition' ? (
        <div className="workflow-inspector-section flex flex-col gap-3 border-t border-secondary/60 pt-4">
          <div className="flex items-start justify-between gap-2">
            <div className="min-w-0">
              <label className="text-xs font-semibold text-high">
                {t('workflow.inspector.routerAgent', {
                  defaultValue: 'Router agent',
                })}
              </label>
              <p className="mt-1 text-xs text-low">
                {t('workflow.inspector.routerSharedNote', {
                  defaultValue: 'Used by all Condition nodes in this workflow.',
                })}
              </p>
            </div>
            <button
              type="button"
              className={secondaryButtonClass}
              disabled={readOnly || !onConfigureRouter}
              onClick={onConfigureRouter}
            >
              <Settings2 className="h-3.5 w-3.5" />
              {t('workflow.inspector.configureRouter', {
                defaultValue: 'Configure',
              })}
            </button>
          </div>

          {hasRouterExecutorConfig ? (
            <div className="flex items-center gap-2 rounded-md border border-brand/20 bg-brand/10 px-2.5 py-2">
              <AgentIcon
                agent={routerDisplay.executor}
                className="size-5 shrink-0"
              />
              <div className="min-w-0">
                <div className="truncate text-xs font-semibold text-high">
                  {routerDisplay.agentLabel}
                </div>
                <div className="truncate text-xs text-low">
                  {routerDisplay.modelLabel}
                </div>
              </div>
            </div>
          ) : (
            <div className="rounded-md border border-warning/40 bg-warning/10 px-2.5 py-2 text-xs text-warning">
              {t('workflow.inspector.routerMissing', {
                defaultValue: 'Router agent is not configured.',
              })}
            </div>
          )}
        </div>
      ) : null}

      {type === 'condition' && hasConditionBranches ? (
        <div className="flex flex-col gap-2">
          <div className="flex items-center justify-between gap-2">
            <label className="text-xs font-semibold text-high">
              {t('workflow.inspector.branches')}
            </label>
            <button
              type="button"
              className={secondaryButtonClass}
              disabled={readOnly}
              onClick={() =>
                handleChange('branches', [
                  ...branches,
                  {
                    id: `branch-${crypto.randomUUID().slice(0, 8)}`,
                    condition: '',
                  },
                ])
              }
            >
              <Plus className="h-3.5 w-3.5" />
              {t('workflow.inspector.addBranch', {
                defaultValue: 'Add branch',
              })}
            </button>
          </div>
          <details className="text-xs leading-relaxed text-low">
            <summary className="cursor-pointer rounded focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-brand">
              {t('workflow.inspector.fieldHelp', {
                defaultValue: 'How it works',
              })}
            </summary>
            <p className="mt-2">
              {t('workflow.inspector.conditionBranchHelp', {
                defaultValue:
                  'Describe each route, then connect its handle to one or more downstream Nodes on the canvas.',
              })}
            </p>
          </details>
          {branches.length === 0 ? (
            <div className="rounded border border-dashed border-secondary/60 p-3 text-xs text-low">
              {t('workflow.inspector.noConditionBranches', {
                defaultValue:
                  'Add a branch to create its semantic connection handle.',
              })}
            </div>
          ) : null}
          {branches.map((branch, index) => {
            const updateBranch = (
              updates: Partial<WorkflowConditionBranch>
            ) => {
              const nextBranches = [...branches];
              nextBranches[index] = {
                ...nextBranches[index],
                ...updates,
              };
              handleChange('branches', nextBranches);
            };
            const targetLabels = graph
              ? graph.edges
                  .filter(
                    (edge) =>
                      edge.source === node.id &&
                      edge.source_handle === `branch:${branch.id}`
                  )
                  .map((edge) => {
                    const target = graph.nodes.find(
                      (candidate) => candidate.id === edge.target
                    );
                    return String(target?.data.display_name ?? edge.target);
                  })
              : [];

            return (
              <div
                key={branch.id ?? branch.target_node_id ?? index}
                title={
                  branch.target_node_id
                    ? t('workflow.inspector.nodeIdTitle', {
                        nodeId: branch.target_node_id,
                        defaultValue: `Node ID: ${branch.target_node_id}`,
                      })
                    : undefined
                }
                className="workflow-inspector-section flex flex-col gap-2 border-t border-secondary/60 pt-3"
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="text-xs font-semibold text-high">
                    {t('workflow.inspector.branchLabel', {
                      index: index + 1,
                      defaultValue: 'Branch {{index}}',
                    })}
                  </span>
                  <button
                    type="button"
                    className={iconButtonClass}
                    aria-label={t('workflow.inspector.removeBranch', {
                      index: index + 1,
                      defaultValue: 'Remove branch {{index}}',
                    })}
                    disabled={readOnly}
                    onClick={() =>
                      handleChange(
                        'branches',
                        branches.filter(
                          (candidate) => candidate.id !== branch.id
                        )
                      )
                    }
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                </div>
                <div className="flex flex-col gap-1">
                  <label className="text-xs font-semibold text-high">
                    {t('workflow.inspector.branchCondition', {
                      defaultValue: 'Route when',
                    })}
                  </label>
                  <textarea
                    className={inputClass}
                    rows={3}
                    value={branch.condition ?? ''}
                    placeholder={t(
                      'workflow.inspector.branchConditionPlaceholder',
                      {
                        defaultValue:
                          'Example: upstream output mentions UI, layout, style, or interaction changes',
                      }
                    )}
                    onChange={(event) =>
                      updateBranch({ condition: event.target.value })
                    }
                    disabled={readOnly}
                  />
                </div>
                <div className="flex flex-col gap-1">
                  <span className="text-xs font-semibold text-high">
                    {t('workflow.inspector.branchTarget', {
                      defaultValue: 'Connected Nodes',
                    })}
                  </span>
                  <div className="rounded-md border border-secondary/60 bg-panel/60 px-2.5 py-2 text-xs text-low">
                    {targetLabels.length > 0
                      ? targetLabels.join(', ')
                      : t('workflow.inspector.branchNotConnected', {
                          defaultValue: 'Not connected yet',
                        })}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      ) : null}

      {type === 'transform' ? (
        <div className="workflow-inspector-section flex flex-col gap-2 border-t border-secondary/60 pt-4">
          <button
            type="button"
            className={secondaryButtonClass}
            aria-expanded={transformTest.open}
            onClick={() => updateTransformTest({ open: !transformTest.open })}
          >
            {t('workflow.inspector.testTransform', {
              defaultValue: 'Test transform',
            })}
          </button>
          {transformTest.open ? (
            <div className="flex flex-col gap-2">
              <label className="text-xs font-semibold text-high">
                {t('workflow.inspector.transformSampleInput', {
                  defaultValue: 'Sample input',
                })}
              </label>
              <textarea
                className={inputClass}
                rows={4}
                value={transformTest.input}
                onChange={(event) =>
                  updateTransformTest({
                    input: event.target.value,
                    result: null,
                  })
                }
              />
              <button
                type="button"
                className={secondaryButtonClass}
                onClick={() =>
                  updateTransformTest({
                    result: applyWorkflowTransform(data, transformTest.input),
                  })
                }
              >
                {t('workflow.inspector.runTransformTest', {
                  defaultValue: 'Run test',
                })}
              </button>
              {transformTest.result ? (
                <div
                  role={transformTest.result.ok ? 'status' : 'alert'}
                  className={
                    transformTest.result.ok
                      ? 'whitespace-pre-wrap rounded-md border border-success/30 bg-success/10 px-3 py-2 text-xs text-high'
                      : 'rounded-md border border-error/30 bg-error/10 px-3 py-2 text-xs text-error'
                  }
                >
                  {transformTest.result.ok
                    ? transformTest.result.output
                    : transformTest.result.error}
                </div>
              ) : null}
            </div>
          ) : null}
        </div>
      ) : null}

      {type === 'arena' && hasArenaAttempts && (
        <>
          <div className="flex items-center justify-between gap-2">
            <label className="text-xs font-semibold text-high">
              {t('workflow.inspector.attempts')}
            </label>
            <button
              type="button"
              className={secondaryButtonClass}
              disabled={readOnly}
              onClick={() =>
                handleChange('attempts', [
                  ...attempts,
                  {
                    id: `attempt-${attempts.length + 1}`,
                    display_name: t('workflow.inspector.attemptLabel', {
                      index: attempts.length + 1,
                    }),
                    prompt_template: '',
                  },
                ])
              }
            >
              <Plus className="h-3.5 w-3.5" />
              {t('workflow.inspector.addAttempt')}
            </button>
          </div>
          {attempts.map((attempt, i) => (
            <div
              key={attempt.id ?? i}
              className="workflow-inspector-section mt-2 flex flex-col gap-2 border-t border-secondary/60 pt-3"
            >
              <div className="flex items-center justify-between gap-2">
                <label className="text-xs font-semibold text-high">
                  {attempt.display_name ??
                    t('workflow.inspector.attemptLabel', { index: i + 1 })}
                </label>
                <button
                  type="button"
                  className={iconButtonClass}
                  aria-label={t('workflow.inspector.removeAttempt', {
                    index: i + 1,
                  })}
                  disabled={readOnly || attempts.length <= 1}
                  onClick={() =>
                    handleChange(
                      'attempts',
                      attempts.filter((_, index) => index !== i)
                    )
                  }
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
              <input
                type="text"
                placeholder={t('workflow.fields.display_name')}
                className={inputClass}
                value={attempt.display_name ?? ''}
                onChange={(e) => {
                  const newAttempts = [...attempts];
                  newAttempts[i] = {
                    ...newAttempts[i],
                    display_name: e.target.value,
                  };
                  handleChange('attempts', newAttempts);
                }}
                disabled={readOnly}
              />
              <WorkflowAgentExecutorField
                value={attempt.executor_config}
                readOnly={readOnly}
                onChange={(executorConfig) => {
                  const newAttempts = [...attempts];
                  newAttempts[i] = {
                    ...newAttempts[i],
                    executor_config: executorConfig,
                  };
                  handleChange('attempts', newAttempts);
                }}
              />
              <textarea
                placeholder={t('workflow.inspector.promptOverride')}
                className={inputClass}
                rows={2}
                value={attempt.prompt_template ?? ''}
                onChange={(e) => {
                  const newAttempts = [...attempts];
                  newAttempts[i] = {
                    ...newAttempts[i],
                    prompt_template: e.target.value,
                  };
                  handleChange('attempts', newAttempts);
                }}
                disabled={readOnly}
              />
            </div>
          ))}
        </>
      )}

      {type !== 'start' && type !== 'end' ? (
        <div className="mt-auto flex flex-col gap-2 border-t border-secondary/60 pt-4">
          <div>
            <div className="text-xs font-semibold text-high">
              {t('workflow.inspector.deleteNode', {
                defaultValue: 'Delete Node',
              })}
            </div>
            <p className="mt-1 text-xs text-low">
              {t('workflow.inspector.deleteNodeDescription', {
                defaultValue:
                  'Remove this Node and its connected Edges from the draft.',
              })}
            </p>
          </div>
          <button
            type="button"
            className="inline-flex w-full items-center justify-center gap-2 rounded-md border border-error/30 bg-error/10 px-3 py-2 font-semibold text-error transition-colors hover:bg-error/15 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={readOnly || !onDelete}
            onClick={() => onDelete?.(node.id)}
          >
            <Trash2 className="h-4 w-4" aria-hidden="true" />
            {t('workflow.inspector.deleteNode', {
              defaultValue: 'Delete Node',
            })}
          </button>
        </div>
      ) : null}
    </div>
  );
}
