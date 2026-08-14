import { createContext, type ReactNode, type RefObject, useContext } from "react";

type ContainerRef = RefObject<HTMLElement | null>;

const PortalContainerContext = createContext<ContainerRef | undefined>(undefined);

export function PortalContainerProvider({
  container,
  children,
}: {
  container: ContainerRef;
  children: ReactNode;
}) {
  return (
    <PortalContainerContext.Provider value={container}>{children}</PortalContainerContext.Provider>
  );
}

export function usePortalContainer(): ContainerRef | undefined {
  return useContext(PortalContainerContext);
}
