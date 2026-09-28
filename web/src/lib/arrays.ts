import { create } from "zustand";
import labels from "../generated/labels.json";
import { omitNodes } from "./byNode";
import type {
  ArrayFailure,
  ArrayStatus,
  CalPhase,
  ProcessorGate,
  ProcessorStatus,
  ServerEvent,
  SyncState,
} from "./types";

export interface ArrayStore {
  byNode: Readonly<Record<string, ArrayStatus>>;
  receivedAt: Readonly<Record<string, number>>;
  observe: (event: ServerEvent) => void;
  seed: (statuses: readonly ArrayStatus[]) => void;
  forget: (nodes: readonly string[]) => void;
  reset: () => void;
}

export const useArrayStore = create<ArrayStore>((set) => ({
  byNode: {},
  receivedAt: {},
  observe: (event) => {
    if (event.type !== "ArrayUpdate") {
      return;
    }
    const status = event.data.status;
    set((state) => ({
      byNode: { ...state.byNode, [status.node]: status },
      receivedAt: { ...state.receivedAt, [status.node]: Date.now() },
    }));
  },
  seed: (statuses) => {
    const now = Date.now();
    set({
      byNode: Object.fromEntries(statuses.map((status) => [status.node, status])),
      receivedAt: Object.fromEntries(statuses.map((status) => [status.node, now])),
    });
  },
  forget: (nodes) =>
    set((state) => ({
      byNode: omitNodes(state.byNode, nodes),
      receivedAt: omitNodes(state.receivedAt, nodes),
    })),
  reset: () => set({ byNode: {}, receivedAt: {} }),
}));

export const SYNC_TEXT: Readonly<Record<SyncState, string>> = labels.sync;

export const CAL_TEXT: Readonly<Record<CalPhase, string>> = labels.cal;

export const GATE_TEXT: Readonly<Record<ProcessorGate, string>> = labels.gate;

const FAILURE_TEXT: Readonly<Record<ArrayFailure["kind"], string>> = labels.failure;

export function failureText(failure: ArrayFailure): string {
  const template = FAILURE_TEXT[failure.kind];
  switch (failure.kind) {
    case "lane_gap":
    case "duplicate_lane":
    case "device_down":
    case "low_coherence":
      return template.replace("{n}", String(failure.lane + 1));
    case "lane_held":
      return template.replace("{n}", String(failure.lane + 1)).replace("{by}", failure.by);
    case "clock_drift":
      return template.replace("{ppm}", failure.ppm.toFixed(1));
    case "geometry_mismatch":
      return template
        .replace("{p}", String(failure.positions))
        .replace("{l}", String(failure.lanes));
    default:
      return template;
  }
}

export function processorStatusOf(
  status: ArrayStatus | undefined,
  node: string,
): ProcessorStatus | null {
  return status?.processors?.find((processor) => processor.node === node) ?? null;
}
