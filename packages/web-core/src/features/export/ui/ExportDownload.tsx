import { useState, useEffect, useCallback, useRef } from 'react';
import { CheckCircleIcon, DownloadSimpleIcon } from '@phosphor-icons/react';
import { Button } from '@vibe/ui/components/Button';
import { ErrorState, LoadingState } from '@vibe/ui/components/StateSurface';

export interface ExportRequest {
  organization_id: string;
  project_ids: string[];
  include_attachments: boolean;
}

interface ExportDownloadProps {
  orgId: string;
  projectIds: string[];
  includeAttachments: boolean;
  onExportMore: () => void;
  exportFn: (request: ExportRequest) => Promise<Response>;
}

export function ExportDownload({
  orgId,
  projectIds,
  includeAttachments,
  onExportMore,
  exportFn,
}: ExportDownloadProps) {
  const [isExporting, setIsExporting] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [downloadUrl, setDownloadUrl] = useState<string | null>(null);
  const [filename, setFilename] = useState('vibe-studio-export.zip');
  const hasStartedRef = useRef(false);

  const startExport = useCallback(async () => {
    setIsExporting(true);
    setError(null);
    setDownloadUrl(null);

    try {
      const response = await exportFn({
        organization_id: orgId,
        project_ids: projectIds,
        include_attachments: includeAttachments,
      });

      if (!response.ok) {
        throw new Error(`Export failed (${response.status})`);
      }

      let downloadFilename = 'vibe-studio-export.zip';
      const disposition = response.headers.get('content-disposition');
      if (disposition) {
        const match = disposition.match(/filename="?([^"]+)"?/);
        if (match) {
          downloadFilename = match[1];
        }
      }
      setFilename(downloadFilename);

      const blob = await response.blob();
      const url = URL.createObjectURL(blob);
      setDownloadUrl(url);

      const a = document.createElement('a');
      a.href = url;
      a.download = downloadFilename;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Export failed');
    } finally {
      setIsExporting(false);
    }
  }, [orgId, projectIds, includeAttachments, exportFn]);

  useEffect(() => {
    if (hasStartedRef.current) {
      return;
    }
    hasStartedRef.current = true;
    void startExport();
  }, [startExport]);

  useEffect(() => {
    return () => {
      if (downloadUrl) {
        URL.revokeObjectURL(downloadUrl);
      }
    };
  }, [downloadUrl]);

  const handleManualDownload = () => {
    if (downloadUrl) {
      const a = document.createElement('a');
      a.href = downloadUrl;
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
    }
  };

  return (
    <div className="p-double space-y-double">
      {isExporting && (
        <LoadingState
          title="Generating your export…"
          description={`This may take a moment${
            includeAttachments ? ', especially with attachments' : ''
          }.`}
        />
      )}

      {error && (
        <ErrorState
          title="Export failed"
          description={error}
          action={
            <Button
              variant="outline"
              onClick={() => {
                hasStartedRef.current = false;
                void startExport();
              }}
            >
              Retry
            </Button>
          }
        />
      )}

      {!isExporting && !error && downloadUrl && (
        <div className="space-y-double">
          <div className="flex flex-col items-center gap-base py-base">
            <CheckCircleIcon
              className="size-icon-lg text-success"
              weight="fill"
            />
            <div className="text-center space-y-half">
              <p className="text-sm font-medium text-high">Export complete!</p>
              <p className="text-xs text-low">
                Your download should start automatically. If not, click the
                button below.
              </p>
            </div>
          </div>

          <div className="space-y-base">
            <button
              onClick={handleManualDownload}
              type="button"
              className="min-h-11 w-full flex items-center justify-center gap-half rounded-sm bg-brand px-base py-half text-sm font-medium text-white hover:bg-brand/90 transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-brand sm:min-h-8"
            >
              <DownloadSimpleIcon className="size-icon-sm" />
              Download {filename}
            </button>

            <button
              type="button"
              onClick={onExportMore}
              className="min-h-11 w-full rounded-sm border border-border bg-secondary px-base py-half text-sm font-medium text-normal hover:bg-primary transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-brand sm:min-h-8"
            >
              Export more projects
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
