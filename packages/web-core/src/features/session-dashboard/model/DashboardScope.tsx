import { createContext, useContext, type ReactNode } from 'react';

export interface DashboardScope {
  hostId: string | null;
  scopeKey: string;
  deployment: 'local' | 'remote';
  navigateToRoute(route: string): void;
}

const DashboardScopeContext = createContext<DashboardScope | null>(null);

export function DashboardScopeProvider({
  value,
  children,
}: {
  value: DashboardScope;
  children: ReactNode;
}) {
  return (
    <DashboardScopeContext.Provider value={value}>
      {children}
    </DashboardScopeContext.Provider>
  );
}

export function useDashboardScope() {
  return useContext(DashboardScopeContext);
}
