import { createContext, useContext } from "react";

const FixtureHostContext = createContext<string | null>(null);

export const FixtureHostProvider = FixtureHostContext.Provider;

export function useHostId() {
  return useContext(FixtureHostContext);
}
