// The live decoder stream (PLAN §5, §13). Decoder frames never go through TanStack Query: ADS-B
// alone can run to hundreds of frames a second, and Query is for server state that a WS
// `StateChanged` invalidates. The stored history stays server state (`GET /api/decoderlog`);
// this store is a bounded in-memory tail of what is arriving right now, plus the per-station
// picture the map/table views need.
import { create } from "zustand";
import type {
  DecodedRecord,
  DecodedRecordOf,
  DecoderEvent,
  DecoderEventOf,
  DecoderKind,
  ServerEvent,
} from "./types";

/** Frames kept per decoder kind, oldest dropped first. Far more than any view renders, and it
 * bounds the store to a few MB even with every decoder running at once. */
export const RING_CAPACITY = 2000;

/** Stations retained per decoder kind. `ageOut` is the intended bound, but only the views that
 * show a target table drive it — a POCSAG-only session would otherwise accumulate one entry per
 * distinct pager address for as long as the tab is open. This is the backstop: least recently
 * seen goes first, which is also the one a table would have shown last. */
export const STATION_CAPACITY = 1000;

/** Frames are staged on arrival and published to the store at most this often, so the re-render
 * rate every subscribed component sees is 10 Hz regardless of the decoder's frame rate. */
export const FLUSH_MS = 100;

/** The latest picture of one emitter, accumulated across frames. */
export interface Station {
  kind: DecoderKind;
  /** Emitter identity within the decoder: ICAO address, MMSI, source callsign, pager RIC. */
  id: string;
  /** Fields merged forward: ADS-B splits identity, callsign, altitude and position across
   * different frame types, so a target's row must accumulate rather than replace. */
  event: DecoderEvent;
  /** ms since the epoch, from the record's server-stamped `at`. */
  lastSeen: number;
  freqHz: number;
  deviceSet: number;
  channel: number;
  /** Frames merged into this row — a target seen once is not a target being tracked. */
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
  /** Frames the server reported as dropped (`ServerEvent::DecodedLost`) — the gaps in this
   * store, surfaced rather than silently absent (PLAN §5). */
  lost: number;
  /** Frames published since the last `clear()`; with `lost`, the denominator of a gap readout. */
  received: number;
  /** Stages a frame. Nothing renders until the next flush (at most `FLUSH_MS` later). */
  push: (record: DecodedRecord) => void;
  /** Rebuilds the station picture from what the server heard before this client connected
   * (`ServerEvent::DecodedBacklog`), so a reload does not start with an empty map.
   *
   * Stations only — these records are already in the stored decoder log, and staging them as
   * frames too would show every one of them twice in a log panel that renders the stored page
   * with a live tail on top.
   *
   * A reconnect re-merges a backlog into stations that survived the disconnect, so `frames`
   * counts a handful of records twice per reconnect. Left alone: it reads as "seen once vs.
   * being tracked", and a target the server still remembers is exactly one being tracked. */
  hydrate: (records: readonly DecodedRecord[]) => void;
  reportLost: (count: number) => void;
  /** WS glue: wire once with `socket.addEventListener(useDecodedStore.getState().observe)` —
   * zustand action identities are stable, so the listener never has to be re-registered. */
  observe: (event: ServerEvent) => void;
  /** Publishes staged frames immediately. Called by the flush timer; exposed so a caller that
   * must see a frame synchronously (tests, a panel closing a view) can force it. */
  flush: () => void;
  /** Drops stations unseen for longer than `maxAgeMs`. The UI decides the horizon — an aircraft
   * out of range stops transmitting, it does not announce that it is gone. */
  ageOut: (maxAgeMs: number, nowMs?: number) => void;
  clear: () => void;
}

// Staging lives outside the store on purpose: writing it through `set` would publish — and
// re-render — on every single frame, which is exactly what the flush interval exists to avoid.
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
      // No write, so a periodic age-out tick costs nothing while every target is fresh.
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

  clear: () => {
    pending.length = 0;
    stationIndex.clear();
    cancelFlush();
    set({ frames: {}, stations: {}, lost: 0, received: 0 });
  },
}));

export function useDecodedKind<K extends DecoderKind>(kind: K): readonly DecodedRecordOf<K>[] {
  // Per-kind slices keep their array identity when other decoders produce frames, so a panel
  // re-renders only for the decoder it draws.
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
      // Newest first, mirroring `GET /api/decoderlog`, so a panel renders the live tail and the
      // stored history with the same code. `added` is ours to reverse in place.
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

/** Whether the record belonged to a tracked station and was merged into it. */
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
  // Re-setting an existing key keeps its insertion position, so rows do not jump around a table
  // as frames arrive; ordering is the view's business.
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

/** Drop least-recently-seen stations down to [`STATION_CAPACITY`]. Insertion order is not
 * recency (a re-set key keeps its position, deliberately, so table rows do not jump), so the
 * victim is chosen by `lastSeen`. */
function evictOldest(stations: Map<string, Station>): void {
  if (stations.size <= STATION_CAPACITY) {
    return;
  }
  const byAge = [...stations.entries()].sort((a, b) => a[1].lastSeen - b[1].lastSeen);
  for (const [id] of byAge.slice(0, stations.size - STATION_CAPACITY)) {
    stations.delete(id);
  }
}

/** `null` for decoders whose output is a stream of independent messages rather than a target
 * being tracked. */
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
    // RDS accretes into one picture per transmitter, identified by its PI code once received.
    case "rds":
      return event.data.pi ?? null;
    // Message-shaped decoders name an emitter in the log, but they do not *accumulate* into a
    // picture of it the way a tracked target does — each NAVTEX broadcast, ACARS block and
    // sub-GHz burst stands alone, and merging them forward would splice unrelated messages
    // into one row. They render as lists of frames instead.
    case "navtex":
    case "acars":
    case "subghz":
    case "rtty":
    case "morse":
    // Subaudible signalling describes the channel, not a station on it — the transmitter it
    // belongs to is whoever is keying up right now, and nothing in the event names them.
    case "tone":
      return null;
  }
}

/** A frame that does not carry a field must not erase what an earlier frame established — an
 * ADS-B position frame has no callsign — so absent and null both mean "unchanged", not "gone". */
function mergeForward(previous: DecoderEvent, next: DecoderEvent): DecoderEvent {
  const data: Record<string, unknown> = { ...previous.data };
  for (const [key, value] of Object.entries(next.data)) {
    if (value != null) {
      data[key] = value;
    }
  }
  return { kind: next.kind, data } as DecoderEvent;
}

/** A record whose timestamp the server could not stamp must not be born already stale. */
function recordTime(record: DecodedRecord): number {
  const at = Date.parse(record.at);
  return Number.isNaN(at) ? Date.now() : at;
}

function snapshotStations(kind: DecoderKind): readonly Station[] {
  const stations = stationIndex.get(kind);
  return stations === undefined ? NO_STATIONS : [...stations.values()];
}

// Keying by a runtime `kind` erases the kind-to-payload correlation TypeScript tracks in
// `FramesByKind`/`StationsByKind`. Every write funnels through these two, and the values always
// come from a record of that same kind, so the invariant holds where it cannot be expressed.
function assignFrames(target: FramesByKind, kind: DecoderKind, value: readonly DecodedRecord[]) {
  (target as Record<string, readonly DecodedRecord[]>)[kind] = value;
}

function assignStations(target: StationsByKind, kind: DecoderKind, value: readonly Station[]) {
  (target as Record<string, readonly Station[]>)[kind] = value;
}
