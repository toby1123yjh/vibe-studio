import { useCallback, useEffect, useMemo, useState } from 'react';
import { ThemeMode } from 'shared/types';
import { useTheme } from '@/shared/hooks/useTheme';
import { ExportLayout } from '@/features/export/ui/ExportLayout';
import type { ExportRequest } from '@/features/export/ui/ExportDownload';
import type {
  ExportOrganization,
  ExportProject,
} from '@/features/export/ui/ExportChooseProjects';
import { useAuth } from '@/shared/hooks/auth/useAuth';
import { useUserOrganizations } from '@/shared/hooks/useUserOrganizations';
import { useOrganizationProjects } from '@/shared/hooks/useOrganizationProjects';
import { makeRequest as makeRemoteRequest } from '@/shared/lib/remoteApi';
import { OAuthDialog } from '@/shared/dialogs/global/OAuthDialog';
import { Button } from '@vibe/ui/components/Button';
import {
  LoadingState,
  PermissionState,
} from '@vibe/ui/components/StateSurface';

function resolveTheme(theme: ThemeMode): 'light' | 'dark' {
  if (theme === ThemeMode.SYSTEM) {
    return window.matchMedia('(prefers-color-scheme: dark)').matches
      ? 'dark'
      : 'light';
  }
  return theme === ThemeMode.DARK ? 'dark' : 'light';
}

interface ExportPageProps {
  exportFn: (request: ExportRequest) => Promise<Response>;
  organizations: ExportOrganization[];
  orgsLoading: boolean;
  orgsError: unknown;
  onRetryOrganizations: () => void;
  projects: ExportProject[];
  projectsLoading: boolean;
  projectsError: unknown;
  onRetryProjects: () => void;
  selectedOrgId: string | null;
  onOrgChange: (orgId: string) => void;
}

export function ExportPage({
  exportFn,
  organizations,
  orgsLoading,
  orgsError,
  onRetryOrganizations,
  projects,
  projectsLoading,
  projectsError,
  onRetryProjects,
  selectedOrgId,
  onOrgChange,
}: ExportPageProps) {
  const { theme } = useTheme();

  const logoSrc =
    resolveTheme(theme) === 'dark'
      ? '/vibe-kanban-logo-dark.svg'
      : '/vibe-kanban-logo.svg';

  return (
    <div className="h-full overflow-auto bg-primary">
      <div className="mx-auto flex min-h-full w-full max-w-3xl flex-col justify-center px-base py-double">
        <div className="rounded-sm border border-border bg-secondary p-double space-y-double">
          <header className="space-y-double text-center">
            <div className="flex justify-center">
              <img
                src={logoSrc}
                alt="Vibe Studio"
                className="h-8 w-auto logo"
              />
            </div>
            <p className="text-sm text-low">
              Download your project and task data to CSV files. Optionally
              downloads your file attachments too.
            </p>
          </header>
          <ExportLayout
            exportFn={exportFn}
            organizations={organizations}
            orgsLoading={orgsLoading}
            orgsError={orgsError}
            onRetryOrganizations={onRetryOrganizations}
            projects={projects}
            projectsLoading={projectsLoading}
            projectsError={projectsError}
            onRetryProjects={onRetryProjects}
            selectedOrgId={selectedOrgId}
            onOrgChange={onOrgChange}
          />
        </div>
      </div>
    </div>
  );
}

export function ExportPageContainer() {
  const { isLoaded, isSignedIn } = useAuth();
  const organizationsQuery = useUserOrganizations();
  const {
    data: orgsData,
    isLoading: orgsLoading,
    error: orgsError,
    refetch: retryOrganizations,
  } = organizationsQuery;
  const organizations = useMemo<ExportOrganization[]>(
    () =>
      (orgsData?.organizations ?? []).map((organization) => ({
        id: organization.id,
        name: organization.name,
      })),
    [orgsData?.organizations]
  );
  const [selectedOrgId, setSelectedOrgId] = useState<string | null>(null);

  useEffect(() => {
    if (organizations.length === 0) {
      return;
    }

    const hasSelectedOrg = selectedOrgId
      ? organizations.some((organization) => organization.id === selectedOrgId)
      : false;

    if (!hasSelectedOrg) {
      setSelectedOrgId(organizations[0].id);
    }
  }, [organizations, selectedOrgId]);

  const effectiveSelectedOrgId =
    selectedOrgId &&
    organizations.some((organization) => organization.id === selectedOrgId)
      ? selectedOrgId
      : (organizations[0]?.id ?? null);

  const {
    data: projectData = [],
    isLoading: projectsLoading,
    error: projectsError,
    retry: retryProjects,
  } = useOrganizationProjects(effectiveSelectedOrgId);
  const projects = useMemo<ExportProject[]>(
    () =>
      projectData.map((project) => ({
        id: project.id,
        name: project.name,
      })),
    [projectData]
  );

  const exportFn = useCallback(async (request: ExportRequest) => {
    return makeRemoteRequest('/v1/export', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(request),
    });
  }, []);

  if (!isLoaded) {
    return (
      <LoadingState
        className="h-full w-full bg-primary"
        title="Loading export access…"
      />
    );
  }

  if (!isSignedIn) {
    return (
      <PermissionState
        className="h-full w-full bg-primary"
        title="Sign in to export your cloud data"
        description="Sign in to choose the organizations and projects available to your account."
        action={
          <Button variant="outline" onClick={() => void OAuthDialog.show({})}>
            Sign in
          </Button>
        }
      />
    );
  }

  return (
    <ExportPage
      exportFn={exportFn}
      organizations={organizations}
      orgsLoading={orgsLoading}
      orgsError={orgsError}
      onRetryOrganizations={() => void retryOrganizations()}
      projects={projects}
      projectsLoading={projectsLoading}
      projectsError={projectsError}
      onRetryProjects={retryProjects}
      selectedOrgId={effectiveSelectedOrgId}
      onOrgChange={setSelectedOrgId}
    />
  );
}
