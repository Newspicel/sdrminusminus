import { useStore } from "@xyflow/react";
import { useMemo } from "react";

const BASE_STEP = 24;
const COARSE_BELOW = 0.5;
const FINE_FROM = 1.25;

export function gridStep(zoom: number): number {
  if (zoom < COARSE_BELOW) {
    return BASE_STEP * 2;
  }
  if (zoom >= FINE_FROM) {
    return BASE_STEP / 2;
  }
  return BASE_STEP;
}

export function useGridStep(): { step: number; snap: [number, number] } {
  const step = useStore((state) => gridStep(state.transform[2]));
  return useMemo(() => ({ step, snap: [step, step] }), [step]);
}
