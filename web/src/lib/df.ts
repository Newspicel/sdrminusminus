import { create } from "zustand";
import type { CalState, DfFusionState, DfReading, RadarDetection, ServerEvent } from "./types";

/// How many bearings a node keeps for the map's fading trail.
export const BEARING_HISTORY = 64;

export interface BearingSample {
  bearingDeg: number;
  confidence: number;
  at: number;
  lat?: number;
  lon?: number;
}

export interface DfNodeState {
  deviceSet: number;
  reading: DfReading;
  cal: CalState;
  at: number;
  history: BearingSample[];
  fusion?: DfFusionState;
  detections?: RadarDetection[];
}

export interface DfStoreState {
  byNode: Readonly<Record<string, DfNodeState>>;
  observe: (event: ServerEvent) => void;
  forget: (node: string) => void;
  reset: () => void;
}

function pushSample(history: readonly BearingSample[], sample: BearingSample): BearingSample[] {
  const next = [...history, sample];
  return next.length > BEARING_HISTORY ? next.slice(next.length - BEARING_HISTORY) : next;
}

const EMPTY_CAL: CalState = { tier: "none", lanes: [], phase_unknown: true, solved: false };

export const useDfStore = create<DfStoreState>((set) => ({
  byNode: {},
  observe: (event: ServerEvent) => {
    if (event.type === "DfUpdate") {
      const { node, device_set, reading, cal } = event.data;
      set((state) => {
        const previous = state.byNode[node];
        const at = Date.now();
        const history =
          reading.confidence > 0
            ? pushSample(previous?.history ?? [], {
                bearingDeg: reading.bearing_deg,
                confidence: reading.confidence,
                at,
              })
            : (previous?.history ?? []);
        return {
          byNode: {
            ...state.byNode,
            [node]: {
              ...previous,
              deviceSet: device_set,
              reading,
              cal,
              at,
              history,
            },
          },
        };
      });
      return;
    }
    if (event.type === "DfFusionUpdate") {
      const { node, state: fusion } = event.data;
      set((state) => {
        const previous = state.byNode[node];
        return {
          byNode: {
            ...state.byNode,
            [node]: {
              deviceSet: previous?.deviceSet ?? 0,
              reading: previous?.reading ?? {
                bearing_deg: 0,
                confidence: 0,
                peak_to_floor_db: 0,
                pseudospectrum: [],
              },
              cal: previous?.cal ?? EMPTY_CAL,
              at: previous?.at ?? Date.now(),
              history: previous?.history ?? [],
              detections: previous?.detections,
              fusion,
            },
          },
        };
      });
      return;
    }
    if (event.type === "RadarDetections") {
      const { node, device_set, detections } = event.data;
      set((state) => {
        const previous = state.byNode[node];
        return {
          byNode: {
            ...state.byNode,
            [node]: {
              deviceSet: device_set,
              reading: previous?.reading ?? {
                bearing_deg: 0,
                confidence: 0,
                peak_to_floor_db: 0,
                pseudospectrum: [],
              },
              cal: previous?.cal ?? EMPTY_CAL,
              at: Date.now(),
              history: previous?.history ?? [],
              fusion: previous?.fusion,
              detections,
            },
          },
        };
      });
    }
  },
  forget: (node: string) =>
    set((state) => {
      if (!(node in state.byNode)) {
        return state;
      }
      const { [node]: _dropped, ...rest } = state.byNode;
      return { byNode: rest };
    }),
  reset: () => set({ byNode: {} }),
}));
