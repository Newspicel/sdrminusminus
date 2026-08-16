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

export const SPECTRUM_HISTORY_ROWS = 1024;

const RELEASE_GRACE_MS = 5_000;

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
      this.bins = frame.bins.length;
      this.ring = new Uint8Array(this.bins * SPECTRUM_HISTORY_ROWS);
      this.meta = [];
      this.write = 0;
      this.filled = 0;
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
  listeners: Set<(frame: SpectrumFrame) => void>;
  history: History;
  release: number;
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
    for (const listener of lane.listeners) {
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
  ): () => void {
    const key = laneKey(deviceSet, stream);
    let lane = this.lanes.get(key);
    if (lane === undefined) {
      lane = { listeners: new Set(), history: new History(), release: 0 };
      this.lanes.set(key, lane);
      this.send({ deviceSet, stream }, true);
    } else if (lane.release !== 0) {
      clearTimeout(lane.release);
      lane.release = 0;
    }
    lane.listeners.add(listener);
    return () => this.release(key, { deviceSet, stream }, listener);
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
    if (watched.listeners.size > 0 || watched.release !== 0) {
      return;
    }
    watched.release = setTimeout(() => {
      this.lanes.delete(key);
      this.send(lane, false);
    }, RELEASE_GRACE_MS);
  }

  private send(lane: Lane, on: boolean): void {
    this.socket?.send(
      on
        ? {
            type: "SubscribeSpectrum",
            data: {
              device_set: lane.deviceSet,
              fps: SPECTRUM_FPS,
              bins: SPECTRUM_BINS,
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
