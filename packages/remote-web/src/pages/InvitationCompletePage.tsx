import { useEffect, useState } from "react";
import { useParams, useSearch } from "@tanstack/react-router";
import { acceptInvitation, redeemOAuth } from "@remote/shared/lib/api";
import { storeTokens } from "@remote/shared/lib/auth";
import {
  clearInvitationToken,
  clearVerifier,
  retrieveInvitationToken,
  retrieveVerifier,
} from "@remote/shared/lib/pkce";
import { StandaloneStatePage } from "@remote/shared/components/StandaloneStatePage";
import { Button } from "@vibe/ui/components/Button";

export default function InvitationCompletePage() {
  const { token: urlToken } = useParams({
    from: "/invitations/$token/complete",
  });
  const search = useSearch({ from: "/invitations/$token/complete" });
  const [error, setError] = useState<string | null>(null);
  const [isAccepted, setIsAccepted] = useState(false);

  const handoffId = search.handoff_id;
  const appCode = search.app_code;
  const oauthError = search.error;

  useEffect(() => {
    const completeInvitation = async () => {
      if (oauthError) {
        setError(`OAuth error: ${oauthError}`);
        return;
      }

      if (!handoffId || !appCode) {
        setError("Invitation callback is incomplete. Please try again.");
        return;
      }

      try {
        const verifier = retrieveVerifier();
        if (!verifier) {
          setError("OAuth session lost. Please try again.");
          return;
        }

        const token = retrieveInvitationToken() || urlToken;
        if (!token) {
          setError("Invitation token lost. Please try again.");
          return;
        }

        const { access_token, refresh_token } = await redeemOAuth(
          handoffId,
          appCode,
          verifier,
        );

        await storeTokens(access_token, refresh_token);
        await acceptInvitation(token, access_token);

        clearVerifier();
        clearInvitationToken();

        setIsAccepted(true);
      } catch (e) {
        setError(
          e instanceof Error ? e.message : "Failed to complete invitation",
        );
        clearVerifier();
        clearInvitationToken();
      }
    };

    void completeInvitation();
  }, [handoffId, appCode, oauthError, urlToken]);

  if (error) {
    const retryPath = urlToken ? `/invitations/${urlToken}/accept` : "/account";

    return (
      <StandaloneStatePage
        state="error"
        title={<h1>Could not accept invitation</h1>}
        description={error}
        action={
          <Button
            className="min-h-11"
            size="lg"
            onClick={() => {
              window.location.assign(retryPath);
            }}
          >
            Try again
          </Button>
        }
      />
    );
  }

  if (isAccepted) {
    return (
      <main className="flex min-h-[100dvh] items-center justify-center overflow-auto bg-primary px-base py-double">
        <section className="w-full max-w-md rounded-sm border border-border bg-secondary p-double text-center">
          <h1 className="text-lg font-semibold text-high">
            Invitation accepted!
          </h1>
          <p className="mt-base text-sm text-normal">
            Your invitation is confirmed. You can now close this page.
          </p>
          <Button asChild className="mt-double min-h-11" size="lg">
            <a
              href="https://github.com/toby1123yjh/vibe-studio/blob/main/docs/getting-started.mdx"
              target="_blank"
              rel="noopener noreferrer"
            >
              Get started
            </a>
          </Button>
        </section>
      </main>
    );
  }

  return (
    <StandaloneStatePage
      state="loading"
      title={<h1>Completing invitation</h1>}
      description="Confirming your account and organization access."
    />
  );
}
