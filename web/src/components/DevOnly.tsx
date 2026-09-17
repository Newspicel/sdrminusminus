import type { ReactNode } from "react";

export function DevOnly({ children }: { children: ReactNode }) {
  return import.meta.env.DEV ? children : null;
}
