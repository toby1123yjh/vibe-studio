import { useMemo } from "react";
import { useParams } from "@tanstack/react-router";
import { useSettingsNavigation } from "@/shared/hooks/useSettingsNavigation";
import { Button } from "@vibe/ui/components/Button";
import {
  EmptyState,
  LoadingState,
  OfflineState,
} from "@vibe/ui/components/StateSurface";

interface BlockedHostState {
  id: string;
  name: string | null;
  errorMessage?: string | null;
}

interface WorkspacesUnavailablePageProps {
  blockedHost?: BlockedHostState;
  isCheckingBlockedHost?: boolean;
  isRetrying?: boolean;
  onRetry?: () => void;
}

export default function WorkspacesUnavailablePage({
  blockedHost,
  isCheckingBlockedHost = false,
  isRetrying = false,
  onRetry,
}: WorkspacesUnavailablePageProps) {
  const { hostId } = useParams({ strict: false });
  const { openSettings } = useSettingsNavigation();

  const selectedHostId = useMemo(
    () => blockedHost?.id ?? hostId ?? null,
    [blockedHost?.id, hostId],
  );

  const selectedHostName = useMemo(
    () => blockedHost?.name ?? selectedHostId,
    [blockedHost?.name, selectedHostId],
  );

  const openRelaySettings = () => {
    openSettings("relay", { hostId: selectedHostId });
  };

  const surfaceClassName =
    "w-full rounded-[var(--vk-radius-md)] border border-[var(--vk-border-subtle)] bg-[var(--vk-surface-secondary)]";

  return (
    <div className="mx-auto flex h-full w-full max-w-3xl items-center justify-center px-double py-double">
      {isCheckingBlockedHost ? (
        <LoadingState
          className={surfaceClassName}
          title={<h1>Connecting to host</h1>}
          description={`Checking ${selectedHostName ?? "the selected host"}.`}
        />
      ) : blockedHost ? (
        <OfflineState
          className={surfaceClassName}
          title={
            <h1>
              Could not connect to {selectedHostName ?? "the selected host"}
            </h1>
          }
          description={
            <div className="space-y-base text-left">
              <p>
                This host is offline or no longer reachable from this browser.
              </p>
              <ol className="list-inside list-decimal space-y-half">
                <li>
                  On that machine, open Vibe Studio and confirm the host is
                  online.
                </li>
                <li>
                  If it still fails, open Relay Settings and pair this host
                  again.
                </li>
              </ol>
              {blockedHost.errorMessage && (
                <p className="break-all text-xs text-low">
                  Last connection error: {blockedHost.errorMessage}
                </p>
              )}
            </div>
          }
          action={
            <div className="flex flex-col gap-[var(--vk-space-2)] sm:flex-row">
              {onRetry && (
                <Button
                  className="min-h-11"
                  size="lg"
                  loading={isRetrying}
                  loadingLabel="Retrying host connection"
                  onClick={onRetry}
                >
                  Retry connection
                </Button>
              )}
              <Button
                className="min-h-11"
                size="lg"
                variant="secondary"
                onClick={openRelaySettings}
              >
                Open Relay Settings
              </Button>
            </div>
          }
        />
      ) : (
        <EmptyState
          className={surfaceClassName}
          title={<h1>No host selected</h1>}
          description="Select or pair an online host to load its workspaces through Relay."
          action={
            <Button
              className="min-h-11"
              size="lg"
              variant="secondary"
              onClick={openRelaySettings}
            >
              Open Relay Settings
            </Button>
          }
        />
      )}
    </div>
  );
}
