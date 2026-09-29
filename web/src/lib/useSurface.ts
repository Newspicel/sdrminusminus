import { useCallback, useEffect, useLayoutEffect, useRef, useSyncExternalStore } from "react";
import { refusalText, type SurfaceFrame, surfaceHub } from "./surface";
import type { SurfaceFit } from "./types";

function watchRefusals(notify: () => void): () => void {
  return surfaceHub.watchRefusals(notify);
}

export function useSurfaceRefusal(node: string | null): string | null {
  const refusal = useCallback(() => (node === null ? null : surfaceHub.refusal(node)), [node]);
  const reason = useSyncExternalStore(watchRefusals, refusal, refusal);
  return reason === null ? null : refusalText(reason);
}

export function useSurface(
  node: string | null,
  onFrame: (frame: SurfaceFrame) => void,
  fit?: SurfaceFit,
): string | null {
  const handler = useRef(onFrame);
  const size = useRef(fit);
  useLayoutEffect(() => {
    handler.current = onFrame;
    size.current = fit;
  });
  useEffect(() => {
    if (node === null) {
      return;
    }
    return surfaceHub.subscribe(node, (frame) => handler.current(frame), size.current);
  }, [node]);
  return useSurfaceRefusal(node);
}
