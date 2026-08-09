// Is the viewport too narrow for a docked layout? A workspace authored on a desktop is
// unusable at phone width, and dockview's minimum panel sizes would rewrite it into something
// the desktop then inherits (PLAN §10: the UI must stay usable on a phone).
import { useSyncExternalStore } from "react";

/** Tailwind's `md` breakpoint, which the rest of the UI already switches on. */
const NARROW = "(max-width: 767px)";

export function useNarrow(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => window.matchMedia(NARROW).matches,
    () => false,
  );
}

function subscribe(onChange: () => void): () => void {
  const query = window.matchMedia(NARROW);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}
