import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/Dialog';
import { Button } from '@vibe/ui/components/Button';
import {
  EmptyState,
  ErrorState,
  LoadingState,
} from '@vibe/ui/components/StateSurface';
import { ExternalLink } from 'lucide-react';
import { create, useModal } from '@ebay/nice-modal-react';
import { defineModal, type NoProps } from '@/shared/lib/modals';
import { useReleases } from '@/shared/hooks/useReleases';
import { SimpleMarkdown } from '@/shared/components/SimpleMarkdown';
import { useTranslation } from 'react-i18next';
import { formatLocalizedDateTime } from '@/shared/lib/date';

const GITHUB_RELEASES_URL =
  'https://github.com/toby1123yjh/vibe-studio/releases';

function formatDate(dateStr: string): string {
  try {
    return formatLocalizedDateTime(dateStr, {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
    });
  } catch {
    return dateStr;
  }
}

function extractVersion(tagName: string): string {
  return tagName.replace(/-\d{14}$/, '');
}

const ReleaseNotesDialogImpl = create<NoProps>(() => {
  const { t } = useTranslation('common');
  const modal = useModal();
  const {
    data: releases,
    isLoading,
    isError,
    isFetching,
    refetch,
  } = useReleases();

  const handleClose = () => {
    modal.hide();
    modal.resolve();
  };

  const handleOpenInBrowser = () => {
    window.open(GITHUB_RELEASES_URL, '_blank', 'noopener,noreferrer');
  };

  return (
    <Dialog
      open={modal.visible}
      onOpenChange={(open) => !open && handleClose()}
    >
      <DialogContent className="flex h-[min(720px,calc(100dvh-2rem))] max-h-[calc(100dvh-2rem)] w-[calc(100%-2rem)] max-w-2xl flex-col overflow-hidden p-0">
        <DialogHeader className="flex-shrink-0 border-b px-6 pb-4 pt-5">
          <DialogTitle className="text-lg font-semibold text-high">
            {t('releaseNotes.title')}
          </DialogTitle>
          <DialogDescription className="sr-only">
            {t('releaseNotes.description')}
          </DialogDescription>
        </DialogHeader>

        <div className="flex-1 space-y-6 overflow-y-auto px-6 py-4 scrollbar-thin">
          {isLoading && (
            <LoadingState
              className="min-h-64"
              title={t('releaseNotes.loading')}
              description={t('releaseNotes.loadingDescription')}
            />
          )}

          {isError && (
            <ErrorState
              className="min-h-64"
              title={t('releaseNotes.failed')}
              description={t('releaseNotes.retryDescription')}
              action={
                <Button
                  className="min-h-11 sm:min-h-8"
                  variant="outline"
                  size="sm"
                  loading={isFetching}
                  loadingLabel={t('releaseNotes.retrying')}
                  onClick={() => {
                    void refetch();
                  }}
                >
                  {t('buttons.retry')}
                </Button>
              }
            />
          )}

          {!isLoading && !isError && releases?.length === 0 && (
            <EmptyState
              className="min-h-64"
              title={t('releaseNotes.empty')}
              description={t('releaseNotes.emptyDescription')}
            />
          )}

          {!isLoading &&
            !isError &&
            releases?.map((release) => (
              <article key={release.tag_name} className="space-y-1.5">
                <div className="flex items-baseline gap-2">
                  <h2 className="text-sm font-semibold text-high">
                    {extractVersion(release.tag_name)}
                  </h2>
                  <span className="text-xs text-low">
                    {formatDate(release.published_at)}
                  </span>
                </div>
                {release.body && (
                  <SimpleMarkdown
                    content={release.body}
                    className="space-y-1.5 pl-0.5"
                  />
                )}
              </article>
            ))}
        </div>

        <DialogFooter className="flex-shrink-0 border-t px-6 py-3">
          <DialogClose asChild>
            <Button className="min-h-11 sm:min-h-8" variant="ghost" size="sm">
              {t('buttons.close')}
            </Button>
          </DialogClose>
          <Button
            className="min-h-11 sm:min-h-8"
            variant="outline"
            size="sm"
            onClick={handleOpenInBrowser}
          >
            <ExternalLink className="mr-1.5 h-3.5 w-3.5" />
            {t('releaseNotes.openGitHub')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
});

export const ReleaseNotesDialog = defineModal<void, void>(
  ReleaseNotesDialogImpl
);
