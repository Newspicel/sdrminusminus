import { useEffect, useLayoutEffect, useRef, useState } from "react";

export function useDebouncedCommit(
  commit: (value: number) => void,
  delayMs = 150,
): { pending: number | null; change: (value: number) => void; cancel: () => void } {
  const [pending, setPending] = useState<number | null>(null);
  const pendingRef = useRef<number | null>(null);
  const commitRef = useRef(commit);
  useLayoutEffect(() => {
    commitRef.current = commit;
  });
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
      setPending(null);
      commitRef.current(value);
    }, delayMs);
  };

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
