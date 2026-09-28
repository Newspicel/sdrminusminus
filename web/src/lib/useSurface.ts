import { useEffect, useLayoutEffect, useRef } from "react";
import { type SurfaceFrame, surfaceHub } from "./surface";
import type { SurfaceFit } from "./types";

export function useSurface(
  node: string,
  onFrame: (frame: SurfaceFrame) => void,
  fit?: SurfaceFit,
): void {
  const handler = useRef(onFrame);
  const size = useRef(fit);
  useLayoutEffect(() => {
    handler.current = onFrame;
    size.current = fit;
  });
  useEffect(
    () => surfaceHub.subscribe(node, (frame) => handler.current(frame), size.current),
    [node],
  );
}
