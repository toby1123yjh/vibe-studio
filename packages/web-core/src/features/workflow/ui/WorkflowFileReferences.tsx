import { useLayoutEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { Download, FileText } from 'lucide-react';
import type { WorkflowFileContent, WorkspaceFileContent } from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/Dialog';
import { createWorkflowApi } from '@/shared/lib/workflowApi';
import { workspacesApi } from '@/shared/lib/api';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { classifyWorkspaceFile } from '@/shared/lib/fileClassification';
import { scopeWorkspaceFileRawUrl } from '@/shared/lib/workspaceFileRawUrl';
import { CodeTextViewer } from '@/shared/components/file-viewers/CodeTextViewer';
import { MarkdownFileViewer } from '@/shared/components/file-viewers/MarkdownFileViewer';
import { ImageFileViewer } from '@/shared/components/file-viewers/ImageFileViewer';

export interface WorkflowDisplayFileReference {
  path: string;
  sha256?: string;
}

export type WorkflowFilePreviewTarget =
  | { kind: 'workflow'; runId: string; path: string }
  | {
      kind: 'workspace';
      workspaceId: string;
      repoId: string;
      path: string;
    };

/** The same shared viewers serve the existing dialog and the inline float. */
export function WorkflowFilePreview({
  target,
  initialScrollTop = 0,
  onScrollTopChange,
}: {
  target: WorkflowFilePreviewTarget;
  initialScrollTop?: number;
  onScrollTopChange?: (top: number) => void;
}) {
  const { t } = useTranslation('common');
  const hostId = useHostId();
  const api = createWorkflowApi(hostId);
  const viewerRef = useRef<HTMLDivElement>(null);
  const savedScrollTop = useRef(initialScrollTop);
  const preview = useQuery<WorkflowFileContent | WorkspaceFileContent>({
    queryKey: ['workflow-file-content', hostId, target],
    queryFn: ({ signal }) =>
      target.kind === 'workflow'
        ? api.fileContent(target.runId, target.path, signal)
        : workspacesApi.files.content(target.workspaceId, {
            repoId: target.repoId,
            path: target.path,
          }),
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
  const displayInfo = preview.data ? classifyWorkspaceFile(preview.data) : null;
  const rawUrl =
    target.kind === 'workflow'
      ? api.fileRawUrl(target.runId, target.path)
      : scopeWorkspaceFileRawUrl(preview.data?.raw_url, hostId);
  useLayoutEffect(() => {
    const viewer = viewerRef.current?.firstElementChild;
    if (preview.data && viewer instanceof HTMLElement) {
      viewer.scrollTop = savedScrollTop.current;
    }
  }, [preview.data]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-start justify-between gap-half border-b border-secondary p-base">
        <p className="min-w-0 text-xs text-low">
          {t('workflow.split.currentFileHint')}
        </p>
        {rawUrl ? (
          <Button asChild variant="outline" size="xs">
            <a href={rawUrl} download>
              <Download className="mr-half h-3.5 w-3.5" aria-hidden="true" />
              {t('workflow.split.downloadFile')}
            </a>
          </Button>
        ) : null}
      </div>
      {preview.data?.truncated ? (
        <p className="shrink-0 border-b border-secondary p-half text-xs text-warning">
          {t('workflow.split.fileTruncated')}
        </p>
      ) : null}
      <div
        ref={viewerRef}
        className="min-h-0 flex-1 overflow-hidden"
        onScrollCapture={(event) => {
          const element = event.target;
          if (
            element instanceof HTMLElement &&
            element.scrollHeight > element.clientHeight
          ) {
            savedScrollTop.current = element.scrollTop;
            onScrollTopChange?.(element.scrollTop);
          }
        }}
      >
        {preview.isPending ? (
          <p className="p-base text-sm text-low" role="status">
            {t('workflow.split.loadingFile')}
          </p>
        ) : preview.isError ? (
          <div className="space-y-base p-base text-sm" role="alert">
            <p className="text-error">{t('workflow.split.fileFailed')}</p>
            <p className="break-words text-low">{preview.error.message}</p>
            <Button
              variant="outline"
              size="sm"
              loading={preview.isFetching}
              onClick={() => void preview.refetch()}
            >
              {t('buttons.retry')}
            </Button>
          </div>
        ) : preview.data ? (
          displayInfo?.viewKind === 'markdown' ? (
            <MarkdownFileViewer content={preview.data.content ?? ''} />
          ) : displayInfo?.viewKind === 'image' ? (
            <ImageFileViewer
              key={rawUrl}
              src={rawUrl}
              alt={preview.data.name}
              fallback={
                <p className="p-base text-sm text-error" role="alert">
                  {t('workflow.split.fileFailed')}
                </p>
              }
            />
          ) : displayInfo?.viewKind === 'code' ||
            displayInfo?.viewKind === 'text' ? (
            <CodeTextViewer
              content={preview.data.content ?? ''}
              language={displayInfo.language}
            />
          ) : (
            <p className="p-base text-sm text-low">
              {t('workflow.split.downloadToView')}
            </p>
          )
        ) : null}
      </div>
    </div>
  );
}

/** Read-only current-file access, kept separate from Agent write evidence. */
export function WorkflowFileReferences({
  runId,
  files,
  onOpenFile,
}: {
  runId: string;
  files: WorkflowDisplayFileReference[];
  onOpenFile?: (target: WorkflowFilePreviewTarget) => void;
}) {
  const { t } = useTranslation('common');
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const uniqueFiles = [
    ...new Map(files.map((file) => [file.path, file])).values(),
  ];

  return (
    <>
      <ul className="space-y-half" data-testid="workflow-file-references">
        {uniqueFiles.map((file) => (
          <li key={file.path}>
            <button
              type="button"
              onClick={() => {
                if (onOpenFile) {
                  onOpenFile({ kind: 'workflow', runId, path: file.path });
                } else {
                  setSelectedPath(file.path);
                }
              }}
              className="flex w-full items-start gap-half rounded px-half py-half text-left text-xs text-brand transition-colors hover:bg-secondary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
            >
              <FileText
                className="mt-0.5 h-3.5 w-3.5 shrink-0"
                aria-hidden="true"
              />
              <span className="min-w-0 break-all font-mono">{file.path}</span>
            </button>
          </li>
        ))}
      </ul>
      <Dialog
        open={selectedPath !== null}
        onOpenChange={(open) => {
          if (!open) setSelectedPath(null);
        }}
      >
        <DialogContent
          className="flex h-[min(85vh,640px)] max-w-3xl flex-col"
          closeLabel={t('buttons.close')}
        >
          <DialogHeader>
            <DialogTitle className="break-all pr-6 font-mono text-sm">
              {selectedPath}
            </DialogTitle>
            <DialogDescription>
              {t('workflow.split.currentFileHint')}
            </DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 overflow-hidden rounded border border-secondary">
            {selectedPath ? (
              <WorkflowFilePreview
                target={{ kind: 'workflow', runId, path: selectedPath }}
              />
            ) : null}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
