import { describe, expect, it } from 'vitest';
import {
  getWorkflowNodeCatalogSections,
  isWorkflowNodeAuthorable,
} from './workflowNodeCatalog';

describe('workflow node catalog sections', () => {
  it('groups node catalog entries into authoring sections', () => {
    const sections = getWorkflowNodeCatalogSections().map((section) => ({
      id: section.id,
      label: section.label,
      labelKey: section.labelKey,
      types: section.entries.map((entry) => entry.type),
    }));

    expect(sections).toEqual([
      {
        id: 'execution',
        label: 'Execution',
        labelKey: 'workflow.editor.sections.execution',
        types: ['agent', 'task_splitter', 'arena'],
      },
      {
        id: 'control',
        label: 'Control',
        labelKey: 'workflow.editor.sections.control',
        types: ['condition', 'human_gate', 'transform'],
      },
    ]);
    expect(sections.flatMap((section) => section.types)).not.toContain('start');
    expect(sections.flatMap((section) => section.types)).not.toContain('end');
  });

  it('keeps structural nodes out of manual authoring', () => {
    expect(isWorkflowNodeAuthorable('agent')).toBe(true);
    expect(isWorkflowNodeAuthorable('condition')).toBe(true);
    expect(isWorkflowNodeAuthorable('human_gate')).toBe(true);
    expect(isWorkflowNodeAuthorable('start')).toBe(false);
    expect(isWorkflowNodeAuthorable('end')).toBe(false);
    expect(isWorkflowNodeAuthorable('task_splitter')).toBe(true);
    expect(isWorkflowNodeAuthorable('transform')).toBe(true);
    expect(isWorkflowNodeAuthorable('arena')).toBe(true);
  });
});
