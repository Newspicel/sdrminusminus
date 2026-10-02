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
  tuning: Readonly<Record<string, number>>;
  tunes: Readonly<Record<string, PendingTune>>;
  observe: (event: ServerEvent) => void;
  seed: (statuses: readonly ArrayStatus[]) => void;
  retune: (node: string, centerHz: number) => void;
  tuned: (node: string, centerHz: number | null) => void;
  forget: (nodes: readonly string[]) => void;
  reset: () => void;
}

export interface PendingTune {
  left: readonly number[];
  settledAt: number | null;
}

export const STALE_REPORT_MS = 2000;

type TuneState = Pick<ArrayStore, "tuning" | "tunes">;

function dropTune(state: TuneState, node: string): TuneState {
  return { tuning: omitNodes(state.tuning, [node]), tunes: omitNodes(state.tunes, [node]) };
}

function reportedStale(tune: PendingTune, centerHz: number, now: number): boolean {
  return (
    tune.settledAt === null ||
    (tune.left.includes(centerHz) && now - tune.settledAt < STALE_REPORT_MS)
  );
}

function confirmTunes(state: TuneState, statuses: readonly ArrayStatus[], now: number): TuneState {
  let next = state;
  for (const status of statuses) {
    const wanted = next.tuning[status.node];
    const tune = next.tunes[status.node];
    if (wanted === undefined || tune === undefined) {
      continue;
    }
    if (status.center_hz === wanted || !reportedStale(tune, status.center_hz, now)) {
      next = dropTune(next, status.node);
    }
  }
  return next;
}

export const useArrayStore = create<ArrayStore>((set) => ({
  byNode: {},
  receivedAt: {},
  tuning: {},
  tunes: {},
  observe: (event) => {
    if (event.type !== "ArrayUpdate") {
      return;
    }
    const status = event.data.status;
    const now = Date.now();
    set((state) => ({
      ...confirmTunes(state, [status], now),
      byNode: { ...state.byNode, [status.node]: status },
      receivedAt: { ...state.receivedAt, [status.node]: now },
    }));
  },
  seed: (statuses) => {
    const now = Date.now();
    set((state) => ({
      ...confirmTunes(state, statuses, now),
      byNode: Object.fromEntries(statuses.map((status) => [status.node, status])),
      receivedAt: Object.fromEntries(statuses.map((status) => [status.node, now])),
    }));
  },
  retune: (node, centerHz) =>
    set((state) => {
      const shown = shownCenterHz(state, node);
      const left = state.tunes[node]?.left ?? [];
      return {
        tuning: { ...state.tuning, [node]: centerHz },
        tunes: {
          ...state.tunes,
          [node]: {
            left: shown === undefined || shown === centerHz ? left : [...left, shown],
            settledAt: null,
          },
        },
      };
    }),
  tuned: (node, centerHz) =>
    set((state) => {
      const status = state.byNode[node];
      const tune = state.tunes[node];
      if (centerHz === null || status === undefined || status.center_hz === centerHz) {
        return dropTune(state, node);
      }
      return {
        tuning: { ...state.tuning, [node]: centerHz },
        tunes: {
          ...state.tunes,
          [node]: { left: tune?.left ?? [status.center_hz], settledAt: Date.now() },
        },
        byNode: { ...state.byNode, [node]: { ...status, center_hz: centerHz } },
      };
    }),
  forget: (nodes) =>
    set((state) => ({
      byNode: omitNodes(state.byNode, nodes),
      receivedAt: omitNodes(state.receivedAt, nodes),
      tuning: omitNodes(state.tuning, nodes),
      tunes: omitNodes(state.tunes, nodes),
    })),
  reset: () => set({ byNode: {}, receivedAt: {}, tuning: {}, tunes: {} }),
}));

export function shownCenterHz(
  state: Pick<ArrayStore, "byNode" | "tuning">,
  node: string,
): number | undefined {
  return state.tuning[node] ?? state.byNode[node]?.center_hz;
}

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
