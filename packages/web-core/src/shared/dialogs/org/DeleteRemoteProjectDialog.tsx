import { useRef, useState } from 'react';
import { Button } from '@vibe/ui/components/Button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/KeyboardDialog';
import { Alert, AlertDescription } from '@vibe/ui/components/Alert';
import { Loader2 } from 'lucide-react';
import { create, useModal } from '@ebay/nice-modal-react';
import { useTranslation } from 'react-i18next';
import { defineModal } from '@/shared/lib/modals';
import { isLocalRemoteApiEnabled } from '@/shared/lib/remoteApi';

export interface DeleteRemoteProjectDialogProps {
  projectName: string;
  onDelete(): Promise<void>;
}

export type DeleteRemoteProjectResult = 'deleted' | 'canceled';

const DeleteRemoteProjectDialogImpl = create<DeleteRemoteProjectDialogProps>(
  ({ projectName, onDelete }) => {
    const modal = useModal();
    const { t } = useTranslation(['projects', 'common']);
    const [isDeleting, setIsDeleting] = useState(false);
    const deletingRef = useRef(false);
    const [error, setError] = useState<string | null>(null);
    const isLocalProject = isLocalRemoteApiEnabled();

    const handleDelete = async () => {
      if (deletingRef.current) return;
      deletingRef.current = true;
      setIsDeleting(true);
      setError(null);

      try {
        await onDelete();
        modal.resolve('deleted' as DeleteRemoteProjectResult);
        modal.hide();
      } catch {
        setError(
          t(
            'deleteProjectDialog.error',
            'Failed to delete project. Please try again.'
          )
        );
      } finally {
        deletingRef.current = false;
        setIsDeleting(false);
      }
    };

    const handleCancel = () => {
      if (deletingRef.current) return;
      modal.resolve('canceled' as DeleteRemoteProjectResult);
      modal.hide();
    };

    const handleOpenChange = (open: boolean) => {
      if (!open) {
        handleCancel();
      }
    };

    return (
      <Dialog
        open={modal.visible}
        onOpenChange={handleOpenChange}
        uncloseable={isDeleting}
        role="dialog"
        aria-modal="true"
        aria-label={t('deleteProjectDialog.title', 'Delete Project?')}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {t('deleteProjectDialog.title', 'Delete Project?')}
            </DialogTitle>
            <DialogDescription>
              {t(
                isLocalProject
                  ? 'deleteProjectDialog.description'
                  : 'deleteProjectDialog.cloudDescription',
                isLocalProject
                  ? 'Remove "{{name}}" from Vibe Studio? Its directory and files will be kept. Selecting this directory again creates a new project; previous tasks and settings will not be restored.'
                  : 'This will permanently delete "{{name}}" and all its tasks. This action cannot be undone.',
                { name: projectName }
              )}
            </DialogDescription>
          </DialogHeader>

          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}

          <DialogFooter>
            <Button
              variant="outline"
              onClick={handleCancel}
              disabled={isDeleting}
            >
              {t('common:buttons.cancel', 'Cancel')}
            </Button>
            <Button
              variant="destructive"
              onClick={handleDelete}
              disabled={isDeleting}
            >
              {isDeleting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {t('common:buttons.delete', 'Delete')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    );
  }
);

export const DeleteRemoteProjectDialog = defineModal<
  DeleteRemoteProjectDialogProps,
  DeleteRemoteProjectResult
>(DeleteRemoteProjectDialogImpl);
