import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';

export function supportsImageInput(
  modalities: readonly string[] | undefined
): boolean | undefined {
  if (!modalities?.length) return undefined;
  return modalities.some((value) => value.toLowerCase() === 'image');
}

/** Image capability never disables normal file uploads or claims unknown support. */
export function useModelAttachmentGuard(
  scope: string,
  upload: (files: File[]) => void
) {
  const { t } = useTranslation('common');
  const [capability, setCapability] = useState<{
    scope: string;
    images: boolean | undefined;
  }>();
  const [notice, setNotice] = useState<{ scope: string; text: string }>();
  const onInputModalitiesChange = useCallback(
    (modalities: string[] | undefined) => {
      const images = supportsImageInput(modalities);
      setCapability((previous) =>
        previous?.scope === scope && previous.images === images
          ? previous
          : { scope, images }
      );
    },
    [scope]
  );
  const images = capability?.scope === scope ? capability.images : undefined;
  const uploadFiles = useCallback(
    (files: File[]) => {
      const isImage = (file: File) =>
        file.type.startsWith('image/') ||
        /\.(png|jpe?g|gif|webp|bmp|avif|svg)$/i.test(file.name);
      const hasImages = files.some(isImage);
      setNotice(
        hasImages && images !== true
          ? {
              scope,
              text: t(
                images === false
                  ? 'modelSelector.imagesUnsupported'
                  : 'modelSelector.imagesUnknown'
              ),
            }
          : undefined
      );
      const accepted =
        images === false ? files.filter((file) => !isImage(file)) : files;
      if (accepted.length) upload(accepted);
    },
    [images, scope, t, upload]
  );
  return {
    onInputModalitiesChange,
    uploadFiles,
    notice: notice?.scope === scope ? notice.text : null,
  };
}
