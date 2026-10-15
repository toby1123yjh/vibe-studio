import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import {
  getInvitation,
  initOAuth,
  type InvitationLookupResponse,
  type OAuthProvider,
} from "@remote/shared/lib/api";
import {
  generateChallenge,
  generateVerifier,
  storeInvitationToken,
  storeVerifier,
} from "@remote/shared/lib/pkce";
import { StandaloneStatePage } from "@remote/shared/components/StandaloneStatePage";
import { Button } from "@vibe/ui/components/Button";
import { StateSurface } from "@vibe/ui/components/StateSurface";

export default function InvitationPage() {
  const { token } = useParams({ from: "/invitations/$token/accept" });
  const [oauthError, setOAuthError] = useState<string | null>(null);
  const [pendingProvider, setPendingProvider] = useState<OAuthProvider | null>(
    null,
  );
  const invitationQuery = useQuery<InvitationLookupResponse>({
    queryKey: ["remote-invitation", token],
    queryFn: () => getInvitation(token),
    retry: false,
  });
  const invitation = invitationQuery.data ?? null;

  const handleOAuthLogin = async (provider: OAuthProvider) => {
    setPendingProvider(provider);
    setOAuthError(null);

    try {
      const verifier = generateVerifier();
      const challenge = await generateChallenge(verifier);

      storeVerifier(verifier);
      storeInvitationToken(token);

      const appBase =
        import.meta.env.VITE_APP_BASE_URL || window.location.origin;
      const callbackUrl = new URL(`/invitations/${token}/complete`, appBase);

      const { authorize_url } = await initOAuth(
        provider,
        callbackUrl.toString(),
        challenge,
      );
      window.location.assign(authorize_url);
    } catch (e) {
      setOAuthError(e instanceof Error ? e.message : "OAuth init failed");
      setPendingProvider(null);
    }
  };

  if (invitationQuery.isError && !invitation) {
    const errorMessage =
      invitationQuery.error instanceof Error
        ? invitationQuery.error.message
        : "Failed to load invitation";

    return (
      <StandaloneStatePage
        state="error"
        title={<h1>Could not load invitation</h1>}
        description={errorMessage}
        action={
          <Button
            className="min-h-11"
            size="lg"
            loading={invitationQuery.isFetching}
            loadingLabel="Retrying invitation lookup"
            onClick={() => void invitationQuery.refetch()}
          >
            Try again
          </Button>
        }
      />
    );
  }

  if (!invitation) {
    return (
      <StandaloneStatePage
        state="loading"
        title={<h1>Loading invitation</h1>}
        description="Checking the invitation details."
      />
    );
  }

  return (
    <main className="flex min-h-[100dvh] items-center overflow-auto bg-primary">
      <div className="mx-auto w-full max-w-md px-base py-double">
        <div className="space-y-double rounded-sm border border-border bg-secondary p-double">
          <header className="space-y-half text-center">
            <h1 className="text-2xl font-semibold text-high">
              You&apos;re invited
            </h1>
            <p className="text-sm text-low">
              You&apos;ve been invited to join{" "}
              <span className="font-medium text-high">
                {invitation.organization_name ?? invitation.organization_slug}
              </span>{" "}
              on Vibe Studio.
            </p>
          </header>

          <section className="mx-auto w-full max-w-xs space-y-half border-t border-border pt-base text-sm">
            <div className="flex items-center justify-between gap-base">
              <span className="text-low">Role</span>
              <span className="font-medium text-high">{invitation.role}</span>
            </div>
            <div className="flex items-center justify-between gap-base">
              <span className="text-low">Expires</span>
              <span className="font-medium text-high">
                {new Date(invitation.expires_at).toLocaleDateString()}
              </span>
            </div>
          </section>

          {oauthError && (
            <StateSurface
              state="error"
              compact
              title="Could not start sign-in"
              description={oauthError}
            />
          )}

          <section className="space-y-base border-t border-border pt-base text-center">
            <p className="text-sm text-low">Choose a provider to continue:</p>
            <div className="flex flex-col items-center gap-2">
              <OAuthButton
                provider="github"
                label="Continue with GitHub"
                onClick={() => void handleOAuthLogin("github")}
                disabled={pendingProvider !== null}
                loading={pendingProvider === "github"}
              />
              <OAuthButton
                provider="google"
                label="Continue with Google"
                onClick={() => void handleOAuthLogin("google")}
                disabled={pendingProvider !== null}
                loading={pendingProvider === "google"}
              />
            </div>
          </section>
        </div>
      </div>
    </main>
  );
}

function OAuthButton({
  provider,
  label,
  onClick,
  disabled,
  loading,
}: {
  provider: OAuthProvider;
  label: string;
  onClick: () => void;
  disabled?: boolean;
  loading?: boolean;
}) {
  return (
    <button
      type="button"
      className="flex min-h-11 min-w-[280px] items-center justify-center rounded-[4px] border border-[#dadce0] bg-[#f2f2f2] px-3 text-[14px] font-medium text-[#1f1f1f] transition-colors hover:bg-[#e8eaed] active:bg-[#e2e3e5] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand disabled:cursor-not-allowed disabled:opacity-50 sm:min-h-8"
      style={{ fontFamily: "'Roboto', Arial, sans-serif" }}
      onClick={onClick}
      disabled={disabled || loading}
    >
      {loading
        ? `Opening ${provider === "github" ? "GitHub" : "Google"}...`
        : label}
    </button>
  );
}
