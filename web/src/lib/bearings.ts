import { create } from "zustand";
import { omitNodes } from "./byNode";
import type { DecodedRecord, ServerEvent } from "./types";

export interface BearingSample {
  node: string;
  trueDeg: number;
  confidence: number;
  sigmaDeg: number;
  lat: number;
  lon: number;
  at: number;
  stationId: string | null;
}

export const BEARING_HISTORY = 64;
export const BEARING_MAX_AGE_MS = 300_000;

export interface BearingStore {
  byNode: Readonly<Record<string, readonly BearingSample[]>>;
  observe: (event: ServerEvent) => void;
  forget: (nodes: readonly string[]) => void;
  reset: () => void;
}

export function bearingOf(record: DecodedRecord, now = Date.now()): BearingSample | null {
  if (record.event.kind !== "df") {
    return null;
  }
  const bearing = record.event.data;
  const node = record.origin?.node ?? bearing.node ?? "";
  const lat = bearing.lat;
  const lon = bearing.lon;
  if (node === "" || !isFiniteNumber(lat) || !isFiniteNumber(lon)) {
    return null;
  }
  const parsed = Date.parse(record.at);
  return {
    node,
    trueDeg: bearing.bearing_deg,
    confidence: bearing.confidence,
    sigmaDeg: bearing.sigma_deg ?? 0,
    lat,
    lon,
    at: Number.isNaN(parsed) ? now : parsed,
    stationId: bearing.station_id ?? null,
  };
}

function isFiniteNumber(value: number | null | undefined): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

export function keepRecent(
  samples: readonly BearingSample[],
  now: number,
): readonly BearingSample[] {
  const cutoff = now - BEARING_MAX_AGE_MS;
  return samples
    .filter((sample) => sample.at >= cutoff)
    .toSorted((a, b) => a.at - b.at)
    .slice(-BEARING_HISTORY);
}

function sameSample(a: BearingSample, b: BearingSample): boolean {
  return (
    a.at === b.at &&
    a.trueDeg === b.trueDeg &&
    a.lat === b.lat &&
    a.lon === b.lon &&
    a.stationId === b.stationId
  );
}

function merge(
  byNode: Readonly<Record<string, readonly BearingSample[]>>,
  records: readonly DecodedRecord[],
): Readonly<Record<string, readonly BearingSample[]>> {
  const now = Date.now();
  const added = new Map<string, BearingSample[]>();
  for (const record of records) {
    const sample = bearingOf(record, now);
    if (sample === null) {
      continue;
    }
    const list = added.get(sample.node) ?? [];
    const held = byNode[sample.node] ?? [];
    if (
      held.some((kept) => sameSample(kept, sample)) ||
      list.some((kept) => sameSample(kept, sample))
    ) {
      continue;
    }
    list.push(sample);
    added.set(sample.node, list);
  }
  if (added.size === 0) {
    return byNode;
  }
  const next: Record<string, readonly BearingSample[]> = { ...byNode };
  for (const [node, samples] of added) {
    next[node] = keepRecent([...(byNode[node] ?? []), ...samples], now);
  }
  return next;
}

export const useBearingStore = create<BearingStore>((set) => ({
  byNode: {},
  observe: (event) => {
    if (event.type === "Decoded") {
      const record = event.data;
      set((state) => ({ byNode: merge(state.byNode, [record]) }));
    } else if (event.type === "DecodedBacklog") {
      const records = event.data.records;
      set((state) => ({ byNode: merge(state.byNode, records) }));
    }
  },
  forget: (nodes) => set((state) => ({ byNode: omitNodes(state.byNode, nodes) })),
  reset: () => set({ byNode: {} }),
}));
