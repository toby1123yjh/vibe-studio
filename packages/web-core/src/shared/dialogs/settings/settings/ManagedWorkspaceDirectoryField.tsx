import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '@vibe/ui/components/Button';
import { Input } from '@vibe/ui/components/Input';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import { FolderPickerDialog } from '@/shared/dialogs/shared/FolderPickerDialog';
import { useSettingsMachineClient } from './SettingsHostContext';
import { useSettingsMachineState } from './SettingsMachineUserSystemProvider';

export function ManagedWorkspaceDirectoryField({
  value,
  onChange,
  disabled = false,
}: {
  value: string | null;
  onChange(value: string | null): void;
  disabled?: boolean;
}) {
  const { t } = useTranslation('settings');
  const machineClient = useSettingsMachineClient();
  const { canMutate } = useSettingsMachineState();
  const [error, setError] = useState<string | null>(null);
  const owner = useRef({
    machineClient,
    enabled: canMutate && !disabled,
    mounted: true,
    epoch: 0,
  });
  if (owner.current.machineClient !== machineClient) owner.current.epoch += 1;
  Object.assign(owner.current, {
    machineClient,
    enabled: canMutate && !disabled,
  });
  useEffect(() => {
    const currentOwner = owner.current;
    currentOwner.mounted = true;
    return () => {
      currentOwner.mounted = false;
      currentOwner.epoch += 1;
    };
  }, []);
  const browsePending = useRef(false);
  const browse = async () => {
    if (!owner.current.enabled || !machineClient || browsePending.current)
      return;
    const epoch = owner.current.epoch;
    browsePending.current = true;
    setError(null);
    try {
      const result = await FolderPickerDialog.show({
        value: value ?? '',
        hostId: machineClient.target.apiHostId,
        title: t('settings.general.managedDirectory.label', {
          defaultValue: 'Default session directory',
        }),
      });
      if (
        result &&
        owner.current.mounted &&
        owner.current.enabled &&
        owner.current.epoch === epoch
      )
        onChange(result);
    } catch {
      if (owner.current.mounted && owner.current.epoch === epoch) {
        setError(
          t('settings.general.managedDirectory.browseFailed', {
            defaultValue: 'Unable to choose a directory. Try again.',
          })
        );
      }
    } finally {
      browsePending.current = false;
    }
  };
  const blocked = disabled || !canMutate || !machineClient;
  return (
    <div className="flex flex-col gap-half">
      <label
        htmlFor="managed-workspace-root"
        className="text-sm font-medium text-normal"
      >
        {t('settings.general.managedDirectory.label', {
          defaultValue: 'Default session directory',
        })}
      </label>
      <div className="flex flex-wrap gap-half">
        <Input
          id="managed-workspace-root"
          value={formatPathForDisplay(value ?? '')}
          title={formatPathForDisplay(value ?? '')}
          onChange={(event) => onChange(event.target.value || null)}
          disabled={blocked}
          placeholder={t('settings.general.managedDirectory.default', {
            defaultValue: 'Automatic (app data / workspaces)',
          })}
          aria-describedby="managed-workspace-root-description"
          className="min-w-0 flex-1"
        />
        <Button
          type="button"
          variant="outline"
          disabled={blocked}
          onClick={() => void browse()}
        >
          {t('settings.general.managedDirectory.browse', {
            defaultValue: 'Browse',
          })}
        </Button>
        <Button
          type="button"
          variant="ghost"
          disabled={blocked || value === null}
          onClick={() => onChange(null)}
        >
          {t('settings.general.managedDirectory.reset', {
            defaultValue: 'Reset',
          })}
        </Button>
      </div>
      <p id="managed-workspace-root-description" className="text-xs text-low">
        {t('settings.general.managedDirectory.description', {
          defaultValue:
            'New standalone sessions get their own folder here. Existing folders and project locations are unchanged.',
        })}
      </p>
      {error && (
        <p role="alert" className="text-xs text-error">
          {error}
        </p>
      )}
    </div>
  );
}
