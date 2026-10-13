import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { Download, FileText } from 'lucide-react';
import { Button } from '@vibe/ui/components/Button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/Dialog';
import { createWorkflowApi } from '@/shared/lib/workflowApi';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { classifyWorkspaceFile } from '@/shared/lib/fileClassification';
import { CodeTextViewer } from '@/shared/components/file-viewers/CodeTextViewer';
import { MarkdownFileViewer } from '@/shared/components/file-viewers/MarkdownFileViewer';
import { ImageFileViewer } from '@/shared/components/file-viewers/ImageFileViewer';

export interface WorkflowDisplayFileReference {
  path: string;
  sha256?: string;
}

/** Read-only current-file access, kept separate from Agent write evidence. */
export function WorkflowFileReferences({
  runId,
  files,
}: {
  runId: string;
  files: WorkflowDisplayFileReference[];
}) {
  const { t } = useTranslation('common');
  const hostId = useHostId();
  const api = createWorkflowApi(hostId);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const preview = useQuery({
    queryKey: ['workflow-file-content', hostId, runId, selectedPath],
    queryFn: ({ signal }) => {
      if (!selectedPath) throw new Error('No workflow file selected');
      return api.fileContent(runId, selectedPath, signal);
    },
    enabled: selectedPath !== null,
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
  const displayInfo = preview.data ? classifyWorkspaceFile(preview.data) : null;
  const rawUrl = selectedPath ? api.fileRawUrl(runId, selectedPath) : null;
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
              onClick={() => setSelectedPath(file.path)}
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
          className="flex max-h-[85vh] max-w-3xl flex-col"
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
          {rawUrl ? (
            <div className="flex justify-end">
              <Button asChild variant="outline" size="sm">
                <a href={rawUrl} download>
                  <Download
                    className="mr-half h-3.5 w-3.5"
                    aria-hidden="true"
                  />
                  {t('workflow.split.downloadFile')}
                </a>
              </Button>
            </div>
          ) : null}
          <div className="min-h-0 overflow-y-auto rounded border border-secondary">
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
              <>
                {preview.data.truncated ? (
                  <p className="border-b border-secondary p-half text-xs text-warning">
                    {t('workflow.split.fileTruncated')}
                  </p>
                ) : null}
                <div className="h-[min(55vh,480px)]">
                  {displayInfo?.viewKind === 'markdown' ? (
                    <MarkdownFileViewer content={preview.data.content ?? ''} />
                  ) : displayInfo?.viewKind === 'image' ? (
                    <ImageFileViewer
                      key={selectedPath}
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
                  )}
                </div>
              </>
            ) : null}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
