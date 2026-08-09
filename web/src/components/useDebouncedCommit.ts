import { useEffect, useRef, useState } from "react";

// Local value while the user drags; the commit is debounced so a drag doesn't flood the
// server. Refs mirror the pending value and latest `commit` so the unmount cleanup (whose
// closure is stale) can flush a pending commit instead of silently dropping the last value.
export function useDebouncedCommit(
  commit: (value: number) => void,
  delayMs = 150,
): { pending: number | null; change: (value: number) => void; cancel: () => void } {
  const [pending, setPending] = useState<number | null>(null);
  const pendingRef = useRef<number | null>(null);
  const commitRef = useRef(commit);
  commitRef.current = commit;
  const timer = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (timer.current !== null) {
        window.clearTimeout(timer.current);
      }
      if (pendingRef.current !== null) {
        commitRef.current(pendingRef.current);
      }
    },
    [],
  );

  const change = (value: number): void => {
    setPending(value);
    pendingRef.current = value;
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
    }
    timer.current = window.setTimeout(() => {
      timer.current = null;
      pendingRef.current = null;
      // The commit updates the query cache synchronously, so clearing the local value in the
      // same tick cannot flash the stale position.
      setPending(null);
      commitRef.current(value);
    }, delayMs);
  };

  // For edits that supersede the slider (e.g. squelch toggled off): without this a still-
  // pending debounced commit would fire afterwards and resurrect the dragged value.
  const cancel = (): void => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
    pendingRef.current = null;
    setPending(null);
  };

  return { pending, change, cancel };
}
