import { create } from "zustand";
import { omitNodes } from "./byNode";
import type { PatchGraph, PositionFix, ServerEvent } from "./types";

const HISTORY_CAPACITY = 5_000;

export interface PositionSample extends PositionFix {
  receivedAt: number;
}

export interface PositionState {
  fix: PositionFix | null;
  error: string | null;
  history: readonly PositionSample[];
}

interface PositionStore {
  sources: Readonly<Record<string, PositionState>>;
  observe: (event: ServerEvent) => void;
  forget: (nodes: readonly string[]) => void;
  clear: () => void;
}

export const usePositionStore = create<PositionStore>((set) => ({
  sources: {},
  observe: (event) => {
    if (event.type !== "PositionChanged") {
      return;
    }
    set((state) => {
      const previous = state.sources[event.data.node] ?? {
        fix: null,
        error: null,
        history: [],
      };
      const fix = event.data.fix ?? null;
      const history =
        fix === null
          ? previous.history
          : appendSample(previous.history, { ...fix, receivedAt: Date.now() });
      return {
        sources: {
          ...state.sources,
          [event.data.node]: { fix, error: event.data.error ?? null, history },
        },
      };
    });
  },
  forget: (nodes) => set((state) => ({ sources: omitNodes(state.sources, nodes) })),
  clear: () => set({ sources: {} }),
}));

function appendSample(
  history: readonly PositionSample[],
  sample: PositionSample,
): readonly PositionSample[] {
  const last = history.at(-1);
  if (
    last !== undefined &&
    last.latitude === sample.latitude &&
    last.longitude === sample.longitude &&
    last.altitude_m === sample.altitude_m
  ) {
    return [...history.slice(0, -1), sample];
  }
  const next = [...history, sample];
  return next.length > HISTORY_CAPACITY ? next.slice(next.length - HISTORY_CAPACITY) : next;
}

export function gridLocator(latitude: number, longitude: number): string {
  const lon = Math.min(359.999_999, Math.max(0, longitude + 180));
  const lat = Math.min(179.999_999, Math.max(0, latitude + 90));
  const lonField = Math.floor(lon / 20);
  const latField = Math.floor(lat / 10);
  const lonSquare = Math.floor((lon % 20) / 2);
  const latSquare = Math.floor(lat % 10);
  const lonSub = Math.floor(((lon % 2) / 2) * 24);
  const latSub = Math.floor((lat % 1) * 24);
  return `${String.fromCharCode(65 + lonField)}${String.fromCharCode(65 + latField)}${lonSquare}${latSquare}${String.fromCharCode(97 + lonSub)}${String.fromCharCode(97 + latSub)}`;
}

export function positionSourcesOf(graph: PatchGraph, node: string): string[] {
  return (graph.edges ?? [])
    .filter((edge) => edge.to.node === node && edge.to.port === "position")
    .map((edge) => edge.from.node);
}
