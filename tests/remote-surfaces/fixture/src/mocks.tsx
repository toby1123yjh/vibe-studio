import type { ReactNode } from 'react';

export type OAuthProvider = 'github' | 'google';

export async function getAuthMethods() {
  return { local_auth_enabled: true, oauth_providers: ['github', 'google'] };
}

export async function localLogin() {
  return { access_token: 'fixture-access', refresh_token: 'fixture-refresh' };
}

export async function initOAuth(provider: OAuthProvider) {
  return { authorize_url: `https://auth.example.test/${provider}` };
}

export async function getInvitation() {
  return {
    id: 'fixture-invitation',
    organization_name: 'A very long fixture organization name for wrapping',
    organization_slug: 'fixture-org',
    role: 'Developer',
    expires_at: '2027-01-01T00:00:00.000Z',
  };
}

export async function listOrganizationProjects(organizationId: string) {
  return [
    {
      id: `${organizationId}-project-a`,
      organization_id: organizationId,
      name: 'A project with a deliberately long title for narrow screens',
      color: '#6b7280',
      sort_order: 0,
      created_at: '2026-01-01T00:00:00.000Z',
      updated_at: '2026-01-02T00:00:00.000Z',
      archived: false,
      task_count: 0,
    },
    {
      id: `${organizationId}-project-b`,
      organization_id: organizationId,
      name: 'Second project',
      color: '#6b7280',
      sort_order: 1,
      created_at: '2026-01-01T00:00:00.000Z',
      updated_at: '2026-01-03T00:00:00.000Z',
      archived: false,
      task_count: 0,
    },
  ];
}

export async function storeTokens() {
  return undefined;
}

export function generateVerifier() {
  return 'fixture-verifier';
}

export async function generateChallenge() {
  return 'fixture-challenge';
}

export function storeVerifier() {
  return undefined;
}

export function storeInvitationToken() {
  return undefined;
}

export function BrandLogo({ className }: { className?: string }) {
  return <span className={className} role="img" aria-label="Vibe Studio" />;
}

export function useSettingsNavigation() {
  return {
    openSettings: () => {
      document.body.dataset.lastNavigation = '/settings';
    },
  };
}

export function useOrganizationStore<T>(
  selector: (state: { setSelectedOrgId: (id: string | null) => void }) => T
) {
  return selector({ setSelectedOrgId: () => undefined });
}

const organizations = [
  {
    id: 'org-fixture',
    name: 'Fixture Organization',
    slug: 'fixture-org',
    is_personal: false,
    task_prefix: 'FIX',
    created_at: '2026-01-01T00:00:00.000Z',
    updated_at: '2026-01-01T00:00:00.000Z',
    user_role: 'owner',
  },
];

export function useUserOrganizations() {
  return {
    data: { organizations },
    isLoading: false,
    isFetching: false,
    error: null,
    refetch: async () => undefined,
  };
}

export function useAuth() {
  return { isSignedIn: true, userId: 'fixture-user' };
}

export function useIsMobile() {
  return window.innerWidth < 640;
}

export function useRelayHosts() {
  return {
    hosts: [
      { id: 'host-fixture', name: 'Fixture Host', status: 'online' as const },
    ],
  };
}

export function resolveRelayNavigationHostId(
  hosts: Array<{ id: string; status: string }>
) {
  return hosts.find((host) => host.status === 'online')?.id ?? null;
}

export function isTauriApp() {
  return false;
}

export function PostHogProvider({ children }: { children: ReactNode }) {
  return children;
}
