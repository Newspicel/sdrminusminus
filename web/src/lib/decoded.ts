import { create } from "zustand";
import type {
  DecodedRecord,
  DecodedRecordOf,
  DecoderEvent,
  DecoderEventOf,
  DecoderKind,
  ServerEvent,
} from "./types";

export const RING_CAPACITY = 2000;

export const STATION_CAPACITY = 1000;

export const FLUSH_MS = 100;

export interface Station {
  kind: DecoderKind;
  id: string;
  event: DecoderEvent;
  lastSeen: number;
  freqHz: number;
  deviceSet: number;
  channel: number;
  frames: number;
}

export type StationOf<K extends DecoderKind> = Omit<Station, "kind" | "event"> & {
  kind: K;
  event: DecoderEventOf<K>;
};

type FramesByKind = { [K in DecoderKind]?: readonly DecodedRecordOf<K>[] };
type StationsByKind = { [K in DecoderKind]?: readonly StationOf<K>[] };

export interface DecodedState {
  frames: FramesByKind;
  stations: StationsByKind;
  lost: number;
  received: number;
  push: (record: DecodedRecord) => void;
  hydrate: (records: readonly DecodedRecord[]) => void;
  reportLost: (count: number) => void;
  observe: (event: ServerEvent) => void;
  flush: () => void;
  ageOut: (maxAgeMs: number, nowMs?: number) => void;
  dropFrames: (match: (record: DecodedRecord) => boolean) => number;
  clear: () => void;
}

const pending: DecodedRecord[] = [];
const stationIndex = new Map<DecoderKind, Map<string, Station>>();
let flushTimer: ReturnType<typeof setTimeout> | null = null;

const NO_FRAMES: readonly never[] = Object.freeze([]);
const NO_STATIONS: readonly never[] = Object.freeze([]);

export const useDecodedStore = create<DecodedState>((set) => ({
  frames: {},
  stations: {},
  lost: 0,
  received: 0,

  push: (record) => {
    pending.push(record);
    if (flushTimer === null) {
      flushTimer = setTimeout(publish, FLUSH_MS);
    }
  },

  hydrate: (records) => {
    const touched = new Set<DecoderKind>();
    for (const record of records) {
      if (mergeStation(record)) {
        touched.add(record.event.kind);
      }
    }
    if (touched.size === 0) {
      return;
    }
    set((state) => {
      const stations = { ...state.stations };
      for (const kind of touched) {
        assignStations(stations, kind, snapshotStations(kind));
      }
      return { stations };
    });
  },

  reportLost: (count) => set((state) => ({ lost: state.lost + count })),

  observe: (event) => {
    if (event.type === "Decoded") {
      useDecodedStore.getState().push(event.data);
    } else if (event.type === "DecodedBacklog") {
      useDecodedStore.getState().hydrate(event.data.records);
    } else if (event.type === "DecodedLost") {
      useDecodedStore.getState().reportLost(event.data.count);
    }
  },

  flush: publish,

  ageOut: (maxAgeMs, nowMs = Date.now()) => {
    const cutoff = nowMs - maxAgeMs;
    const expired: DecoderKind[] = [];
    for (const [kind, stations] of stationIndex) {
      let dropped = false;
      for (const [id, station] of stations) {
        if (station.lastSeen < cutoff) {
          stations.delete(id);
          dropped = true;
        }
      }
      if (dropped) {
        expired.push(kind);
      }
    }
    if (expired.length === 0) {
      return;
    }
    set((state) => {
      const stations = { ...state.stations };
      for (const kind of expired) {
        assignStations(stations, kind, snapshotStations(kind));
      }
      return { stations };
    });
  },

  dropFrames: (match) => {
    const staged = pending.filter((record) => !match(record));
    let dropped = pending.length - staged.length;
    pending.splice(0, pending.length, ...staged);

    const published = useDecodedStore.getState().frames;
    const frames = { ...published };
    let touched = false;
    for (const [kind, slice] of Object.entries(published) as [
      DecoderKind,
      readonly DecodedRecord[],
    ][]) {
      const kept = slice.filter((record) => !match(record));
      if (kept.length !== slice.length) {
        dropped += slice.length - kept.length;
        touched = true;
        assignFrames(frames, kind, kept);
      }
    }
    if (touched) {
      set({ frames });
    }
    return dropped;
  },

  clear: () => {
    pending.length = 0;
    stationIndex.clear();
    cancelFlush();
    set({ frames: {}, stations: {}, lost: 0, received: 0 });
  },
}));

export function useDecodedKind<K extends DecoderKind>(kind: K): readonly DecodedRecordOf<K>[] {
  return useDecodedStore((state) => state.frames[kind] ?? NO_FRAMES);
}

export function useStations<K extends DecoderKind>(kind: K): readonly StationOf<K>[] {
  return useDecodedStore((state) => state.stations[kind] ?? NO_STATIONS);
}

function publish(): void {
  cancelFlush();
  if (pending.length === 0) {
    return;
  }
  const batch = pending.splice(0, pending.length);
  const byKind = new Map<DecoderKind, DecodedRecord[]>();
  for (const record of batch) {
    const kind = record.event.kind;
    let group = byKind.get(kind);
    if (group === undefined) {
      group = [];
      byKind.set(kind, group);
    }
    group.push(record);
    mergeStation(record);
  }

  useDecodedStore.setState((state) => {
    const frames = { ...state.frames };
    const stations = { ...state.stations };
    for (const [kind, added] of byKind) {
      added.reverse();
      const next = added.concat(state.frames[kind] ?? NO_FRAMES);
      if (next.length > RING_CAPACITY) {
        next.length = RING_CAPACITY;
      }
      assignFrames(frames, kind, next);
      if (stationIndex.has(kind)) {
        assignStations(stations, kind, snapshotStations(kind));
      }
    }
    return { frames, stations, received: state.received + batch.length };
  });
}

function cancelFlush(): void {
  if (flushTimer !== null) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }
}

function mergeStation(record: DecodedRecord): boolean {
  const id = stationId(record.event);
  if (id === null) {
    return false;
  }
  const kind = record.event.kind;
  let stations = stationIndex.get(kind);
  if (stations === undefined) {
    stations = new Map();
    stationIndex.set(kind, stations);
  }
  const previous = stations.get(id);
  stations.set(id, {
    kind,
    id,
    event: previous === undefined ? record.event : mergeForward(previous.event, record.event),
    lastSeen: recordTime(record),
    freqHz: record.freq_hz,
    deviceSet: record.device_set,
    channel: record.channel,
    frames: (previous?.frames ?? 0) + 1,
  });
  evictOldest(stations);
  return true;
}

function evictOldest(stations: Map<string, Station>): void {
  if (stations.size <= STATION_CAPACITY) {
    return;
  }
  const byAge = [...stations.entries()].sort((a, b) => a[1].lastSeen - b[1].lastSeen);
  for (const [id] of byAge.slice(0, stations.size - STATION_CAPACITY)) {
    stations.delete(id);
  }
}

function stationId(event: DecoderEvent): string | null {
  switch (event.kind) {
    case "adsb":
      return event.data.icao;
    case "ais":
      return String(event.data.mmsi);
    case "aprs":
      return event.data.source;
    case "pocsag":
      return String(event.data.address);
    case "flex":
      return String(event.data.address);
    case "ermes":
      return String(event.data.local_address);
    case "rds":
      return event.data.pi ?? null;
    case "navtex":
    case "acars":
    case "subghz":
    case "selcall":
    case "rtty":
    case "morse":
    case "cw_skimmer":
    case "psk31":
    case "psk63":
    case "ft8":
    case "ft4":
    case "wspr":
    case "tone":
    case "dv":
    case "ident":
    case "broadcast":
    case "radio_clock":
    case "gnss":
      return null;
  }
}

function mergeForward(previous: DecoderEvent, next: DecoderEvent): DecoderEvent {
  const data: Record<string, unknown> = { ...previous.data };
  for (const [key, value] of Object.entries(next.data)) {
    if (value != null) {
      data[key] = value;
    }
  }
  return { kind: next.kind, data } as DecoderEvent;
}

function recordTime(record: DecodedRecord): number {
  const at = Date.parse(record.at);
  return Number.isNaN(at) ? Date.now() : at;
}

function snapshotStations(kind: DecoderKind): readonly Station[] {
  const stations = stationIndex.get(kind);
  return stations === undefined ? NO_STATIONS : [...stations.values()];
}

function assignFrames(target: FramesByKind, kind: DecoderKind, value: readonly DecodedRecord[]) {
  (target as Record<string, readonly DecodedRecord[]>)[kind] = value;
}

function assignStations(target: StationsByKind, kind: DecoderKind, value: readonly Station[]) {
  (target as Record<string, readonly Station[]>)[kind] = value;
}
