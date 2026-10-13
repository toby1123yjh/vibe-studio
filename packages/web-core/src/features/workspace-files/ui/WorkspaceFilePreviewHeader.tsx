import { useMemo, useState } from 'react';
import {
  Copy,
  Download,
  ExternalLink,
  Loader2,
  RefreshCw,
  X,
} from 'lucide-react';
import { Tooltip } from '@vibe/ui/components/Tooltip';
import { cn, formatFileSize } from '@/shared/lib/utils';
import { writeClipboardViaBridge } from '@/shared/lib/clipboard';
import { getFileIcon } from '@/shared/lib/fileTypeIcon';
import { getActualTheme } from '@/shared/lib/theme';
import { useTheme } from '@/shared/hooks/useTheme';
import { useOpenInEditor } from '@/shared/hooks/useOpenInEditor';
import { useHostId } from '@/shared/providers/HostIdProvider';
import type { WorkspaceFileContent } from 'shared/types';
import type { WorkspaceFilePreviewTarget } from '../model/types';
import type { FileDisplayInfo } from '@/shared/lib/fileClassification';
import { scopeWorkspaceFileRawUrl } from '@/shared/lib/workspaceFileRawUrl';

interface WorkspaceFilePreviewHeaderProps {
  target: WorkspaceFilePreviewTarget;
  content?: WorkspaceFileContent | null;
  displayInfo?: FileDisplayInfo | null;
  isRefreshing?: boolean;
  onRefresh: () => void;
  onClearTarget?: () => void;
}

export function WorkspaceFilePreviewHeader({
  target,
  content,
  displayInfo,
  isRefreshing,
  onRefresh,
  onClearTarget,
}: WorkspaceFilePreviewHeaderProps) {
  const { theme } = useTheme();
  const actualTheme = getActualTheme(theme);
  const hostId = useHostId();
  const [copied, setCopied] = useState(false);
  const openInEditor = useOpenInEditor(target.workspaceId);
  const fileName = content?.name || target.path.split('/').pop() || target.path;
  const repoName = content?.repo_name ?? target.repoId;
  const rawUrl = scopeWorkspaceFileRawUrl(content?.raw_url, hostId);

  const FileIcon = useMemo(
    () => getFileIcon(fileName, actualTheme),
    [actualTheme, fileName]
  );

  const handleCopyPath = async () => {
    await writeClipboardViaBridge(target.path);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  const handleOpenEditor = () => {
    void openInEditor({ filePath: target.path });
  };

  return (
    <div className="sticky top-0 z-10 border-b border-border bg-primary">
      <div className="flex min-h-[48px] items-center gap-base px-base py-half">
        <div className="flex size-8 shrink-0 items-center justify-center rounded border border-border bg-secondary">
          <FileIcon className="size-5" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-half">
            <div className="truncate text-base font-semibold text-high">
              {fileName}
            </div>
            <span className="shrink-0 rounded border border-border bg-secondary px-1.5 py-0.5 text-xs text-low">
              {repoName}
            </span>
          </div>
          <div className="mt-0.5 flex min-w-0 items-center gap-half text-xs text-low">
            <span className="min-w-0 truncate font-ibm-plex-mono">
              {target.path}
            </span>
            {content && (
              <>
                <span className="shrink-0">|</span>
                <span className="shrink-0">
                  {formatFileSize(content.size_bytes)}
                </span>
              </>
            )}
            {displayInfo && (
              <>
                <span className="shrink-0">|</span>
                <span className="shrink-0">{displayInfo.label}</span>
              </>
            )}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-half">
          <HeaderIconButton
            label={copied ? 'Copied path' : 'Copy path'}
            onClick={() => void handleCopyPath()}
          >
            <Copy className="size-4" />
          </HeaderIconButton>
          <HeaderIconButton label="Open in editor" onClick={handleOpenEditor}>
            <ExternalLink className="size-4" />
          </HeaderIconButton>
          <HeaderIconButton
            label="Refresh preview"
            onClick={onRefresh}
            disabled={isRefreshing}
          >
            <RefreshCw
              className={cn('size-4', isRefreshing && 'animate-spin')}
            />
          </HeaderIconButton>
          <Tooltip content={rawUrl ? 'Open raw file' : 'Raw file unavailable'}>
            <a
              href={rawUrl ?? undefined}
              target="_blank"
              rel="noopener noreferrer"
              aria-label="Open raw file"
              className={cn(
                'flex size-8 items-center justify-center rounded border border-border bg-secondary text-low transition-colors hover:text-normal focus:outline-none focus:ring-1 focus:ring-brand',
                !rawUrl && 'pointer-events-none opacity-50'
              )}
            >
              <Download className="size-4" />
            </a>
          </Tooltip>
          {onClearTarget && (
            <HeaderIconButton label="Close preview" onClick={onClearTarget}>
              <X className="size-4" />
            </HeaderIconButton>
          )}
        </div>
      </div>
      {isRefreshing && (
        <div className="flex h-6 items-center gap-half border-t border-border px-base text-xs text-low">
          <Loader2 className="size-3 animate-spin" />
          Refreshing preview
        </div>
      )}
    </div>
  );
}

interface HeaderIconButtonProps {
  label: string;
  disabled?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}

function HeaderIconButton({
  label,
  disabled,
  onClick,
  children,
}: HeaderIconButtonProps) {
  return (
    <Tooltip content={label}>
      <button
        type="button"
        onClick={onClick}
        disabled={disabled}
        aria-label={label}
        className="flex size-8 items-center justify-center rounded border border-border bg-secondary text-low transition-colors hover:text-normal focus:outline-none focus:ring-1 focus:ring-brand disabled:cursor-not-allowed disabled:opacity-50"
      >
        {children}
      </button>
    </Tooltip>
  );
}
