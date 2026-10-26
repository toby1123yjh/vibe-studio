import { useEffect, useRef, useState } from 'react';
import { create, useModal } from '@ebay/nice-modal-react';
import { Loader2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Button } from '@vibe/ui/components/Button';
import { Input } from '@vibe/ui/components/Input';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/KeyboardDialog';
import { fileSystemApi } from '@/shared/lib/api';
import { defineModal } from '@/shared/lib/modals';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { useAppRuntime } from '@/shared/hooks/useAppRuntime';

export interface FolderPickerDialogProps {
  value?: string;
  title?: string;
  description?: string;
  hostId?: string | null;
}

// Progress/error surface only. The operating system owns directory browsing.
const FolderPickerDialogImpl = create<FolderPickerDialogProps>(
  ({ value = '', title, hostId }) => {
    const modal = useModal();
    const { t } = useTranslation('common');
    const routeHostId = useHostId();
    const runtime = useAppRuntime();
    const targetHostId = hostId === undefined ? routeHostId : hostId;
    const remote = runtime === 'remote' || targetHostId !== null;
    const [path, setPath] = useState(value);
    const [error, setError] = useState<string | null>(null);
    const [pending, setPending] = useState(!remote);
    const started = useRef(false);
    const owner = useRef({ mounted: true, targetHostId, runtime, epoch: 0 });
    if (
      owner.current.targetHostId !== targetHostId ||
      owner.current.runtime !== runtime
    )
      owner.current.epoch++;
    Object.assign(owner.current, { targetHostId, runtime });

    useEffect(() => {
      const currentOwner = owner.current;
      currentOwner.mounted = true;
      return () => {
        currentOwner.mounted = false;
      };
    }, []);

    useEffect(() => {
      if (!modal.visible) {
        started.current = false;
        return;
      }
      if (started.current) return;
      started.current = true;
      setPath(value);
      setError(null);
      setPending(!remote);
      if (remote) return;
      const epoch = owner.current.epoch;
      void (async () => {
        try {
          const selected = await fileSystemApi.pickFolder(
            {
              initial_path: value.trim() || null,
              title: title?.trim() || null,
            },
            null
          );
          if (!owner.current.mounted) return;
          modal.resolve(owner.current.epoch === epoch ? selected : null);
          modal.hide();
        } catch (nextError) {
          if (!owner.current.mounted) return;
          setError(
            nextError instanceof Error ? nextError.message : String(nextError)
          );
        } finally {
          if (owner.current.mounted) setPending(false);
        }
      })();
    }, [modal, remote, title, value]);

    const close = (result: string | null) => {
      if (pending) return;
      modal.resolve(result);
      modal.hide();
    };

    return (
      <Dialog
        open={modal.visible}
        uncloseable={pending}
        onOpenChange={(open) => {
          if (!open) close(null);
        }}
      >
        <DialogContent
          role="dialog"
          aria-modal="true"
          aria-labelledby="folder-picker-title"
          className="max-w-lg"
        >
          <DialogHeader>
            <DialogTitle id="folder-picker-title">
              {title ?? t('folderPicker.title')}
            </DialogTitle>
            <DialogDescription>
              {pending
                ? t('folderPicker.nativeWaiting')
                : remote
                  ? t('folderPicker.remotePath')
                  : t('folderPicker.manualFallback')}
            </DialogDescription>
          </DialogHeader>
          {pending ? (
            <div
              role="status"
              className="flex items-center gap-half py-base text-sm text-low"
            >
              <Loader2
                aria-hidden="true"
                className="size-icon-sm animate-spin motion-reduce:animate-none"
              />
              {t('folderPicker.nativeWaiting')}
            </div>
          ) : (
            <>
              {error && (
                <p role="alert" className="text-sm text-error">
                  {error}
                </p>
              )}
              <label className="flex flex-col gap-half text-sm">
                {t('folderPicker.path')}
                <Input
                  value={formatPathForDisplay(path)}
                  title={formatPathForDisplay(path)}
                  onChange={(event) => setPath(event.target.value)}
                  autoFocus
                />
              </label>
              <DialogFooter>
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => close(null)}
                >
                  {t('buttons.cancel')}
                </Button>
                <Button
                  type="button"
                  disabled={!path.trim()}
                  onClick={() => close(path.trim())}
                >
                  {t('folderPicker.select')}
                </Button>
              </DialogFooter>
            </>
          )}
        </DialogContent>
      </Dialog>
    );
  }
);
export const FolderPickerDialog = defineModal<
  FolderPickerDialogProps,
  string | null
>(FolderPickerDialogImpl);
