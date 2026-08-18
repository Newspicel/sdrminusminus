import type { SpectrumFrame } from "./frame";
import type { Listener, Unsubscribe } from "./listeners";
import type { ClientCommand, ServerEvent } from "./types";

export interface SpectrumSocket {
  send(command: ClientCommand): void;
  isConnected(): boolean;
  on<K extends "spectrum" | "status" | "event">(kind: K, listener: Listener<K>): Unsubscribe;
}

export const SPECTRUM_FPS = 30;
export const SPECTRUM_BINS = 1024;
export const SPECTRUM_MAX_BINS = 4096;

export const SPECTRUM_HISTORY_ROWS = 1024;

const RELEASE_GRACE_MS = 5_000;

export function binsForView(width: number): number {
  if (!(width > 0)) {
    return SPECTRUM_BINS;
  }
  let bins = SPECTRUM_BINS;
  while (bins < SPECTRUM_MAX_BINS && bins * width < SPECTRUM_BINS) {
    bins *= 2;
  }
  return bins;
}

export function resampleRows(
  rows: Uint8Array,
  count: number,
  fromBins: number,
  toBins: number,
): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(count * toBins);
  if (fromBins === 0 || toBins === 0) {
    return out;
  }
  for (let row = 0; row < count; row++) {
    const from = row * fromBins;
    const to = row * toBins;
    for (let x = 0; x < toBins; x++) {
      const lo = Math.min(fromBins - 1, Math.floor((x * fromBins) / toBins));
      const hi = Math.min(fromBins - 1, Math.max(lo, Math.ceil(((x + 1) * fromBins) / toBins) - 1));
      let peak = 0;
      for (let i = lo; i <= hi; i++) {
        const value = rows[from + i] ?? 0;
        if (value > peak) {
          peak = value;
        }
      }
      out[to + x] = peak;
    }
  }
  return out;
}

function resolveBins(desired: Iterable<number>): number {
  let top = SPECTRUM_BINS;
  for (const bins of desired) {
    if (bins > top) {
      top = bins;
    }
  }
  return Math.min(SPECTRUM_MAX_BINS, top);
}

export interface Lane {
  deviceSet: number;
  stream: number;
}

export interface RowMeta {
  centerHz: number;
  spanHz: number;
  dbMin: number;
  dbMax: number;
  at: number;
}

export interface SpectrumHistory {
  rows: Uint8Array;
  count: number;
  bins: number;
  meta: RowMeta[];
}

function laneKey(deviceSet: number, stream: number): string {
  return `${deviceSet}:${stream}`;
}

class History {
  private ring = new Uint8Array(0);
  private meta: (RowMeta | undefined)[] = [];
  private bins = 0;
  private write = 0;
  private filled = 0;
  latest: SpectrumFrame | null = null;

  record(frame: SpectrumFrame): void {
    this.latest = frame;
    if (frame.bins.length === 0) {
      return;
    }
    if (frame.bins.length !== this.bins) {
      if (this.filled > 0) {
        this.ring = resampleRows(this.ring, SPECTRUM_HISTORY_ROWS, this.bins, frame.bins.length);
      } else {
        this.ring = new Uint8Array(frame.bins.length * SPECTRUM_HISTORY_ROWS);
        this.meta = [];
        this.write = 0;
        this.filled = 0;
      }
      this.bins = frame.bins.length;
    }
    this.ring.set(frame.bins, this.write * this.bins);
    this.meta[this.write] = {
      centerHz: frame.centerHz,
      spanHz: frame.spanHz,
      dbMin: frame.dbMin,
      dbMax: frame.dbMax,
      at: Date.now(),
    };
    this.write = (this.write + 1) % SPECTRUM_HISTORY_ROWS;
    this.filled = Math.min(this.filled + 1, SPECTRUM_HISTORY_ROWS);
  }

  read(): SpectrumHistory {
    const rows = new Uint8Array(this.filled * this.bins);
    const first = (this.write - this.filled + SPECTRUM_HISTORY_ROWS) % SPECTRUM_HISTORY_ROWS;
    const head = Math.min(this.filled, SPECTRUM_HISTORY_ROWS - first);
    rows.set(this.ring.subarray(first * this.bins, (first + head) * this.bins));
    rows.set(this.ring.subarray(0, (this.filled - head) * this.bins), head * this.bins);
    const meta: RowMeta[] = [];
    for (let i = 0; i < this.filled; i++) {
      const row = this.meta[(first + i) % SPECTRUM_HISTORY_ROWS];
      if (row !== undefined) {
        meta.push(row);
      }
    }
    return { rows, count: this.filled, bins: this.bins, meta };
  }
}

interface Watched {
  listeners: Map<(frame: SpectrumFrame) => void, number>;
  history: History;
  release: number;
  bins: number;
}

export class SpectrumHub {
  private socket: SpectrumSocket | null = null;
  private unsubscribes: Unsubscribe[] = [];
  private readonly lanes = new Map<string, Watched>();
  private readonly ids = new Map<number, string>();

  private readonly onFrame = (frame: SpectrumFrame): void => {
    const key = this.ids.get(frame.streamId);
    const lane = key === undefined ? undefined : this.lanes.get(key);
    if (lane === undefined) {
      return;
    }
    lane.history.record(frame);
    for (const listener of lane.listeners.keys()) {
      listener(frame);
    }
  };

  private readonly onEvent = (event: ServerEvent): void => {
    if (event.type === "StreamStarted") {
      const { stream_id, device_set, stream } = event.data;
      this.ids.set(stream_id, laneKey(device_set, stream ?? 0));
    } else if (event.type === "StreamStopped" && event.data.kind === "spectrum") {
      this.ids.delete(event.data.stream_id);
    }
  };

  private readonly onStatus = (connected: boolean): void => {
    if (!connected) {
      return;
    }
    this.ids.clear();
    for (const lane of this.watched()) {
      this.send(lane, true);
    }
  };

  attach(socket: SpectrumSocket): void {
    if (this.socket === socket) {
      return;
    }
    this.detach();
    this.socket = socket;
    this.unsubscribes = [
      socket.on("spectrum", this.onFrame),
      socket.on("status", this.onStatus),
      socket.on("event", this.onEvent),
    ];
    this.ids.clear();
    for (const lane of this.watched()) {
      this.send(lane, true);
    }
  }

  detach(): void {
    this.socket = null;
    for (const unsubscribe of this.unsubscribes) {
      unsubscribe();
    }
    this.unsubscribes = [];
  }

  subscribe(
    deviceSet: number,
    stream: number,
    listener: (frame: SpectrumFrame) => void,
    bins: number = SPECTRUM_BINS,
  ): () => void {
    const key = laneKey(deviceSet, stream);
    let lane = this.lanes.get(key);
    if (lane === undefined) {
      lane = {
        listeners: new Map([[listener, bins]]),
        history: new History(),
        release: 0,
        bins: resolveBins([bins]),
      };
      this.lanes.set(key, lane);
      this.send({ deviceSet, stream }, true);
      return () => this.release(key, { deviceSet, stream }, listener);
    }
    if (lane.release !== 0) {
      clearTimeout(lane.release);
      lane.release = 0;
    }
    lane.listeners.set(listener, bins);
    this.refresh(key, { deviceSet, stream });
    return () => this.release(key, { deviceSet, stream }, listener);
  }

  setBins(
    deviceSet: number,
    stream: number,
    listener: (frame: SpectrumFrame) => void,
    bins: number,
  ): void {
    const key = laneKey(deviceSet, stream);
    const lane = this.lanes.get(key);
    if (lane === undefined || !lane.listeners.has(listener)) {
      return;
    }
    lane.listeners.set(listener, bins);
    this.refresh(key, { deviceSet, stream });
  }

  history(deviceSet: number, stream: number): SpectrumHistory {
    return (
      this.lanes.get(laneKey(deviceSet, stream))?.history.read() ?? {
        rows: new Uint8Array(0),
        count: 0,
        bins: 0,
        meta: [],
      }
    );
  }

  latest(deviceSet: number, stream: number): SpectrumFrame | null {
    return this.lanes.get(laneKey(deviceSet, stream))?.history.latest ?? null;
  }

  watched(): Lane[] {
    return [...this.lanes.keys()].map((key) => {
      const [deviceSet, stream] = key.split(":");
      return { deviceSet: Number(deviceSet), stream: Number(stream) };
    });
  }

  private release(key: string, lane: Lane, listener: (frame: SpectrumFrame) => void): void {
    const watched = this.lanes.get(key);
    if (watched === undefined) {
      return;
    }
    watched.listeners.delete(listener);
    if (watched.listeners.size > 0) {
      this.refresh(key, lane);
      return;
    }
    if (watched.release !== 0) {
      return;
    }
    watched.release = setTimeout(() => {
      this.lanes.delete(key);
      this.send(lane, false);
    }, RELEASE_GRACE_MS);
  }

  private refresh(key: string, lane: Lane): void {
    const watched = this.lanes.get(key);
    if (watched === undefined) {
      return;
    }
    const resolved = resolveBins(watched.listeners.values());
    if (resolved !== watched.bins) {
      watched.bins = resolved;
      this.send(lane, true);
    }
  }

  private send(lane: Lane, on: boolean): void {
    const bins = this.lanes.get(laneKey(lane.deviceSet, lane.stream))?.bins ?? SPECTRUM_BINS;
    this.socket?.send(
      on
        ? {
            type: "SubscribeSpectrum",
            data: {
              device_set: lane.deviceSet,
              fps: SPECTRUM_FPS,
              bins,
              stream: lane.stream,
            },
          }
        : {
            type: "UnsubscribeSpectrum",
            data: { device_set: lane.deviceSet, stream: lane.stream },
          },
    );
  }
}

export const spectrumHub = new SpectrumHub();
