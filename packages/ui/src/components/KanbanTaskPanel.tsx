import {
  useState,
  useEffect,
  useRef,
  useCallback,
  type ReactNode,
  type RefObject,
} from 'react';
import { useTranslation } from 'react-i18next';
import type { LocalAttachmentMetadata } from './WorkspaceContext';
import { cn } from '../lib/cn';
import {
  XIcon,
  LinkIcon,
  DotsThreeIcon,
  TrashIcon,
  PaperclipIcon,
  ImageIcon,
} from '@phosphor-icons/react';
import {
  TaskTagsRow,
  type TaskTagBase,
  type TaskTagsRowAddTagControlProps,
  type LinkedPullRequest as TaskTagsLinkedPullRequest,
} from './TaskTagsRow';
import { PrimaryButton } from './PrimaryButton';
import { TaskPropertyRow, type TaskPropertyRowProps } from './TaskPropertyRow';
import { IconButton } from './IconButton';
import { AutoResizeTextarea } from './AutoResizeTextarea';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from './RadixTooltip';
import { ErrorAlert } from './ErrorAlert';

export type TaskPanelMode = 'create' | 'edit';
type TaskPriority = TaskPropertyRowProps['priority'];
type TaskStatus = TaskPropertyRowProps['statuses'][number];
type TaskAssignee = NonNullable<TaskPropertyRowProps['assigneeUsers']>[number];
type TaskCreator = Exclude<TaskPropertyRowProps['creatorUser'], undefined>;
export interface KanbanTaskTag extends TaskTagBase {
  project_id: string;
}

export interface TaskFormData {
  title: string;
  description: string | null;
  statusId: string;
  priority: TaskPriority | null;
  assigneeIds: string[];
  tagIds: string[];
}

export interface LinkedPullRequest extends TaskTagsLinkedPullRequest {}

export interface KanbanTaskDescriptionEditorProps {
  placeholder: string;
  value: string;
  onChange: (value: string) => void;
  onCmdEnter?: () => void;
  onPasteFiles?: (files: File[]) => void;
  disabled?: boolean;
  autoFocus?: boolean;
  className?: string;
  localAttachments?: LocalAttachmentMetadata[];
  showStaticToolbar?: boolean;
  saveStatus?: 'idle' | 'saved';
  staticToolbarActions?: ReactNode;
  onRequestEdit?: () => void;
  hideActions?: boolean;
}

export interface KanbanTaskPanelProps {
  mode: TaskPanelMode;
  displayId: string;

  // Form data
  formData: TaskFormData;
  onFormChange: <K extends keyof TaskFormData>(
    field: K,
    value: TaskFormData[K]
  ) => void;

  // Options for dropdowns
  statuses: TaskStatus[];
  tags: KanbanTaskTag[];

  // Resolved assignee profiles for avatar display
  assigneeUsers?: TaskAssignee[];

  // Edit mode data
  taskId?: string | null;
  creatorUser?: TaskCreator;
  parentTask?: { id: string; simpleId: string } | null;
  onParentTaskClick?: () => void;
  onRemoveParentTask?: () => void;
  linkedPrs?: LinkedPullRequest[];
  onLinkPr?: () => void;

  // Actions
  onClose: () => void;
  onSubmit: () => void;
  onCmdEnterSubmit?: () => void;
  onDeleteDraft?: () => void;

  // Tag create callback - returns the new tag ID
  onCreateTag?: (data: { name: string; color: string }) => string;
  renderAddTagControl?: (
    props: TaskTagsRowAddTagControlProps<KanbanTaskTag>
  ) => ReactNode;
  renderDescriptionEditor: (
    props: KanbanTaskDescriptionEditorProps
  ) => ReactNode;
  renderProjectWorkspaceContext?: () => ReactNode;

  // Loading states
  isSubmitting?: boolean;
  submitError?: string | null;
  onDismissSubmitError?: () => void;

  // Save status for description field
  descriptionSaveStatus?: 'idle' | 'saved';

  // Ref for title input (created in container)
  titleInputRef: RefObject<HTMLTextAreaElement>;

  // Copy link callback (edit mode only)
  onCopyLink?: () => void;

  // More actions callback (edit mode only) - opens command bar with issue actions
  onMoreActions?: () => void;

  // Image attachment upload
  onPasteFiles?: (files: File[]) => void;
  localAttachments?: LocalAttachmentMetadata[];
  dropzoneProps?: {
    getRootProps: () => Record<string, unknown>;
    getInputProps: () => Record<string, unknown>;
    isDragActive: boolean;
  };
  onBrowseAttachment?: () => void;
  isUploading?: boolean;
  attachmentError?: string | null;
  onDismissAttachmentError?: () => void;

  // Edit-mode section renderers
  renderWorkspacesSection?: (taskId: string) => ReactNode;
  renderRelationshipsSection?: (taskId: string) => ReactNode;
  renderSubTasksSection?: (taskId: string) => ReactNode;
  renderCommentsSection?: (taskId: string) => ReactNode;
}

export function KanbanTaskPanel({
  mode,
  displayId,
  formData,
  onFormChange,
  statuses,
  tags,
  assigneeUsers,
  taskId,
  creatorUser,
  parentTask,
  onParentTaskClick,
  onRemoveParentTask,
  linkedPrs = [],
  onLinkPr,
  onClose,
  onSubmit,
  onCmdEnterSubmit,
  onDeleteDraft,
  onCreateTag,
  renderAddTagControl,
  renderDescriptionEditor,
  renderProjectWorkspaceContext,
  isSubmitting,
  submitError,
  onDismissSubmitError,
  descriptionSaveStatus,
  titleInputRef,
  onCopyLink,
  onMoreActions,
  onPasteFiles,
  localAttachments,
  dropzoneProps,
  onBrowseAttachment,
  isUploading,
  attachmentError,
  onDismissAttachmentError,
  renderWorkspacesSection,
  renderRelationshipsSection,
  renderSubTasksSection,
  renderCommentsSection,
}: KanbanTaskPanelProps) {
  const { t } = useTranslation('common');
  const isCreateMode = mode === 'create';
  const breadcrumbTextClass =
    'min-w-0 text-sm text-normal truncate rounded-sm px-1 py-0.5 hover:bg-panel hover:text-high transition-colors';
  const creatorName =
    creatorUser?.first_name?.trim() || creatorUser?.username?.trim() || null;
  const showCreator = !isCreateMode && Boolean(creatorName);

  // Description edit state: in edit mode, show preview by default; in create mode, always editable
  const [isDescriptionEditing, setIsDescriptionEditing] =
    useState(isCreateMode);
  const descriptionContainerRef = useRef<HTMLDivElement>(null);

  // Reset description editing state when switching between create/edit mode or when issue changes
  useEffect(() => {
    setIsDescriptionEditing(isCreateMode);
  }, [isCreateMode, taskId]);

  // Click outside the description area to exit editing
  const handleDescriptionBlur = useCallback(() => {
    if (!isCreateMode) {
      setIsDescriptionEditing(false);
    }
  }, [isCreateMode]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      const target = e.target as HTMLElement;
      const isEditable =
        target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.isContentEditable;
      if (isEditable) {
        // If editing description, exit edit mode first
        if (
          isDescriptionEditing &&
          !isCreateMode &&
          descriptionContainerRef.current?.contains(target)
        ) {
          setIsDescriptionEditing(false);
        }
        target.blur();
        (e.currentTarget as HTMLElement).focus();
        e.stopPropagation();
      } else {
        onClose();
      }
    }
  };

  const handleTitleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      onCmdEnterSubmit?.();
    }
  };

  return (
    <div
      className="flex flex-col h-full overflow-hidden outline-none"
      onKeyDown={handleKeyDown}
      tabIndex={-1}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-base py-half border-b shrink-0">
        <div className="flex items-center gap-half min-w-0 font-ibm-plex-mono">
          <span className={`${breadcrumbTextClass} shrink-0`}>{displayId}</span>
          {!isCreateMode && onCopyLink && (
            <button
              type="button"
              onClick={onCopyLink}
              className="p-half rounded-sm text-low hover:text-normal hover:bg-panel transition-colors"
              aria-label={t('kanban.copyLink')}
            >
              <LinkIcon className="size-icon-sm" weight="bold" />
            </button>
          )}
        </div>
        <div className="flex items-center gap-half">
          {!isCreateMode && onMoreActions && (
            <button
              type="button"
              onClick={onMoreActions}
              className="p-half rounded-sm text-low hover:text-normal hover:bg-panel transition-colors"
              aria-label={t('kanban.moreActions')}
            >
              <DotsThreeIcon className="size-icon-sm" weight="bold" />
            </button>
          )}
          <button
            type="button"
            onClick={onClose}
            className="p-half rounded-sm text-low hover:text-normal hover:bg-panel transition-colors"
            aria-label={t('kanban.closePanel')}
          >
            <XIcon className="size-icon-sm" weight="bold" />
          </button>
        </div>
      </div>

      {/* Scrollable Content */}
      <div className="flex-1 overflow-y-auto">
        {/* Property Row */}
        <div className="px-base py-base border-b">
          <TaskPropertyRow
            statusId={formData.statusId}
            priority={formData.priority}
            assigneeIds={formData.assigneeIds}
            assigneeUsers={assigneeUsers}
            statuses={statuses}
            creatorUser={showCreator ? creatorUser : undefined}
            parentTask={parentTask}
            onParentTaskClick={onParentTaskClick}
            onRemoveParentTask={onRemoveParentTask}
            onStatusClick={() => onFormChange('statusId', formData.statusId)}
            onPriorityClick={() => onFormChange('priority', formData.priority)}
            onAssigneeClick={() =>
              onFormChange('assigneeIds', formData.assigneeIds)
            }
            disabled={isSubmitting}
          />
        </div>

        {/* Tags Row */}
        <div className="px-base py-base border-b">
          <TaskTagsRow
            selectedTagIds={formData.tagIds}
            availableTags={tags}
            linkedPrs={isCreateMode ? [] : linkedPrs}
            onTagsChange={(tagIds) => onFormChange('tagIds', tagIds)}
            onCreateTag={onCreateTag}
            renderAddTagControl={renderAddTagControl}
            onLinkPr={!isCreateMode ? onLinkPr : undefined}
            disabled={isSubmitting}
          />
        </div>

        {isCreateMode && renderProjectWorkspaceContext ? (
          <div className="px-base py-base border-b">
            {renderProjectWorkspaceContext()}
          </div>
        ) : null}

        {/* Title and Description */}
        <div className="rounded-sm">
          {/* Title Input */}
          <div className="w-full mt-base">
            <AutoResizeTextarea
              ref={titleInputRef}
              value={formData.title}
              onChange={(value) => onFormChange('title', value)}
              onKeyDown={handleTitleKeyDown}
              placeholder="Task Title..."
              autoFocus={isCreateMode}
              aria-label="Task title"
              disabled={isSubmitting}
              className={cn(
                'px-base text-lg font-medium text-high',
                'placeholder:text-high/50',
                isSubmitting && 'opacity-50 pointer-events-none'
              )}
            />

            <div
              className={cn(
                'pointer-events-none absolute inset-0 px-base',
                'text-high/50 font-medium text-lg',
                'hidden',
                "[[data-empty='true']_+_&]:block" // show placeholder when previous sibling data-empty=true
              )}
            >
              {t('kanban.issueTitlePlaceholder')}
            </div>
          </div>

          {/* Description WYSIWYG Editor with image dropzone */}
          <div
            ref={descriptionContainerRef}
            {...(isDescriptionEditing ? dropzoneProps?.getRootProps() : {})}
            className={cn(
              'relative mt-base',
              !isDescriptionEditing && !isCreateMode && 'cursor-text'
            )}
            onClick={() => {
              if (!isDescriptionEditing && !isCreateMode && !isSubmitting) {
                // Don't enter edit mode if the user was selecting text
                const selection = window.getSelection();
                if (selection && selection.toString().length > 0) return;
                setIsDescriptionEditing(true);
              }
            }}
            onBlur={(e) => {
              // Exit edit mode when focus leaves the description container
              if (
                descriptionContainerRef.current &&
                !descriptionContainerRef.current.contains(
                  e.relatedTarget as Node
                )
              ) {
                handleDescriptionBlur();
              }
            }}
          >
            {isDescriptionEditing && (
              <input
                {...(dropzoneProps?.getInputProps() as React.InputHTMLAttributes<HTMLInputElement>)}
                data-dropzone-input
              />
            )}
            {renderDescriptionEditor({
              placeholder: isDescriptionEditing
                ? t('kanban.issueDescriptionPlaceholder')
                : formData.description
                  ? ''
                  : t('kanban.issueDescriptionPlaceholder'),
              value: formData.description ?? '',
              onChange: (value) => onFormChange('description', value || null),
              onCmdEnter: onCmdEnterSubmit,
              onPasteFiles: isDescriptionEditing ? onPasteFiles : undefined,
              disabled: !isDescriptionEditing || isSubmitting,
              autoFocus: false,
              className: cn(
                'px-base',
                isDescriptionEditing ? 'min-h-[100px]' : 'min-h-[2rem]',
                !isDescriptionEditing && !formData.description && 'text-low'
              ),
              localAttachments,
              showStaticToolbar: !isCreateMode || isDescriptionEditing,
              hideActions: true,
              saveStatus: descriptionSaveStatus,
              onRequestEdit: !isCreateMode
                ? () => setIsDescriptionEditing(true)
                : undefined,
              staticToolbarActions: (
                <>
                  {isDescriptionEditing && onBrowseAttachment && (
                    <TooltipProvider>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            type="button"
                            onMouseDown={(e) => {
                              e.preventDefault();
                              if (!isSubmitting && !isUploading) {
                                onBrowseAttachment();
                              }
                            }}
                            disabled={isSubmitting || isUploading}
                            className={cn(
                              'p-half rounded-sm transition-colors',
                              'text-low hover:text-normal hover:bg-panel/50',
                              'disabled:opacity-50 disabled:cursor-not-allowed'
                            )}
                            title={t('kanban.attachFile')}
                            aria-label={t('kanban.attachFile')}
                          >
                            <PaperclipIcon className="size-icon-sm" />
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>
                          {t('kanban.attachFileHint')}
                        </TooltipContent>
                      </Tooltip>
                    </TooltipProvider>
                  )}
                </>
              ),
            })}
            {attachmentError && (
              <div className="px-base">
                <ErrorAlert
                  message={attachmentError}
                  className="mt-half mb-half"
                  onDismiss={onDismissAttachmentError}
                  dismissLabel={t('buttons.close')}
                />
              </div>
            )}
            {dropzoneProps?.isDragActive && (
              <div className="absolute inset-0 z-50 bg-primary/80 backdrop-blur-sm border-2 border-dashed border-brand rounded flex items-center justify-center pointer-events-none animate-in fade-in-0 duration-150">
                <div className="text-center">
                  <div className="mx-auto mb-2 w-10 h-10 rounded-full bg-brand/10 flex items-center justify-center">
                    <ImageIcon className="h-5 w-5 text-brand" />
                  </div>
                  <p className="text-sm font-medium text-high">
                    {t('kanban.dropFilesHere')}
                  </p>
                  <p className="text-xs text-low mt-0.5">
                    {t('kanban.fileDropHint')}
                  </p>
                </div>
              </div>
            )}
          </div>
        </div>

        {/* Create Issue Button (Create mode only) */}
        {isCreateMode && (
          <div className="px-base pb-base">
            {submitError && (
              <ErrorAlert
                message={submitError}
                className="mb-base"
                onDismiss={onDismissSubmitError}
                dismissLabel={t('buttons.close')}
              />
            )}
            <div className="flex items-center gap-half">
              <PrimaryButton
                value={t('kanban.createIssue')}
                onClick={onSubmit}
                disabled={isSubmitting || isUploading || !formData.title.trim()}
                actionIcon={isSubmitting ? 'spinner' : undefined}
                variant="default"
              />
              {onDeleteDraft && (
                <IconButton
                  icon={TrashIcon}
                  onClick={onDeleteDraft}
                  disabled={isSubmitting}
                  aria-label="Delete draft"
                  title="Delete draft"
                  className="hover:text-error hover:bg-error/10"
                />
              )}
            </div>
          </div>
        )}

        {/* Workspaces Section (Edit mode only) */}
        {!isCreateMode && taskId && renderWorkspacesSection && (
          <div className="border-t">{renderWorkspacesSection(taskId)}</div>
        )}

        {/* Relationships Section (Edit mode only) */}
        {!isCreateMode && taskId && renderRelationshipsSection && (
          <div className="border-t">{renderRelationshipsSection(taskId)}</div>
        )}

        {/* Sub-Issues Section (Edit mode only) */}
        {!isCreateMode && taskId && renderSubTasksSection && (
          <div className="border-t">{renderSubTasksSection(taskId)}</div>
        )}

        {/* Comments Section (Edit mode only) */}
        {!isCreateMode && taskId && renderCommentsSection && (
          <div className="border-t">{renderCommentsSection(taskId)}</div>
        )}
      </div>
    </div>
  );
}
