// One spectrum subscription per (device set, receive stream), however many scope faces watch it.
//
// The canvas can carry several scope nodes, and two of them on one lane must not each send a
// `SubscribeSpectrum` — the server answers the second by replacing the first's stream, and the
// unmount of either would stop the other's feed. So subscription is refcounted here, in one place.
//
// A multi-stream radio is why the key is a pair rather than a device-set id: several lanes of one
// radio can be watched at once, and they are independent subscriptions the server can start and
// stop separately.
//
// Frames carry a stream id the *server* allocates per connection — it is not the device-set id and
// not the lane index — so the id is learned from the `StreamStarted` that answers each subscribe
// and forgotten on `StreamStopped`. That is the same contract the audio engine follows.
//
// Reconnects are the other reason this is not per-component state: subscriptions are
// per-connection, so everything wanted must be re-sent when the socket comes back.
//
// Faces are the third. A scope face is remounted by things that have nothing to do with its radio
// its own GL texture, so that round trip used to leave the operator watching an empty waterfall
// fill in again. So the lane's recent rows are kept here, where they outlive any one face, and the
// stream is stopped a few seconds after the last watcher rather than the instant it lets go.

import type { SpectrumFrame } from "./frame";
import type { ClientCommand, ServerEvent } from "./types";

/** What the hub needs of a socket — structural, so the unit tests need no WebSocket. */
export interface SpectrumSocket {
  send(command: ClientCommand): void;
  isConnected(): boolean;
  addSpectrumListener(listener: (frame: SpectrumFrame) => void): void;
  removeSpectrumListener(listener: (frame: SpectrumFrame) => void): void;
  addStatusListener(listener: (connected: boolean) => void): void;
  removeStatusListener(listener: (connected: boolean) => void): void;
  addEventListener(listener: (event: ServerEvent) => void): void;
  removeEventListener(listener: (event: ServerEvent) => void): void;
}

export const SPECTRUM_FPS = 30;
export const SPECTRUM_BINS = 1024;

/** Rows of past spectrum kept per lane. Sized to the deepest history a plot can draw, so a face
 * that comes back finds the whole waterfall it left and not a band of it. */
export const SPECTRUM_HISTORY_ROWS = 1024;

/** How long a lane's stream and history outlive their last watcher. A view switch unmounts every
 * face and mounts it again in the same commit: stopping the server's stream on the way through
 * would cost a restart, which the operator reads as a plot that stalled. */
const RELEASE_GRACE_MS = 5_000;

/** One watched lane. */
export interface Lane {
  deviceSet: number;
  stream: number;
}

/** What one history row was measured under. The bytes alone are not readable: the server picks
 * each frame's dB window adaptively, and a scrubbed row has to be drawn against its own. */
export interface RowMeta {
  centerHz: number;
  spanHz: number;
  dbMin: number;
  dbMax: number;
  /** Wall clock when the row arrived, the only clock a scrub readout can label rows with — the
   * frame's own timestamp counts samples since the lane started and means nothing to a reader. */
  at: number;
}

/** The waterfall a face that has just mounted opens on: `count` rows of `bins` bytes packed
 * oldest-first, and what each was measured under. Read separately from `latest` because reading
 * it copies the ring, and the trace and the readout want only the one frame. */
export interface SpectrumHistory {
  rows: Uint8Array;
  count: number;
  bins: number;
  /** Oldest-first, `count` long and index-aligned with `rows`. */
  meta: RowMeta[];
}

/** Map key for a lane. The pair is the identity; the id the server allocates is not, because it
 * changes on every resubscribe and is absent until one is answered. */
function laneKey(deviceSet: number, stream: number): string {
  return `${deviceSet}:${stream}`;
}

/** One lane's rows in a ring: retention costs one buffer per lane and no allocation per frame. */
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
    // A different bin count is a different x axis, so the rows already held cannot be drawn above
    // the ones arriving now — the plot reallocates its texture at this same seam.
    if (frame.bins.length !== this.bins) {
      this.bins = frame.bins.length;
      this.ring = new Uint8Array(this.bins * SPECTRUM_HISTORY_ROWS);
      this.meta = [];
      this.write = 0;
      this.filled = 0;
    }
    // Copied, not referenced: `bins` is a view into the socket's message buffer, and holding a
    // thousand of those would pin a thousand messages.
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
    // Unwrapped here rather than by the reader: oldest-first starts one row past the write cursor
    // once the ring has wrapped, and the plot uploads its history in a single call.
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

/** One lane's watchers and the past they share. The entry lives exactly as long as the server's
 * stream does, which outlasts the last watcher by `RELEASE_GRACE_MS`. */
interface Watched {
  listeners: Set<(frame: SpectrumFrame) => void>;
  history: History;
  /** A pending stop, or 0 — non-zero is precisely "subscribed, but nothing is watching". */
  release: number;
}

export class SpectrumHub {
  private socket: SpectrumSocket | null = null;
  private readonly lanes = new Map<string, Watched>();
  /** Server-allocated stream id → lane key, from `StreamStarted`. Frames carry only the id. */
  private readonly ids = new Map<number, string>();

  private readonly onFrame = (frame: SpectrumFrame): void => {
    const key = this.ids.get(frame.streamId);
    const lane = key === undefined ? undefined : this.lanes.get(key);
    if (lane === undefined) {
      return;
    }
    // Recorded before it is delivered, and recorded through the grace as well: a lane whose faces
    // are between mounts is exactly the one whose history has to stay unbroken.
    lane.history.record(frame);
    for (const listener of lane.listeners) {
      listener(frame);
    }
  };

  // Which lane an id carries is only ever stated here: the binary frame header has room for the id
  // and nothing else, so losing this mapping silently blanks every waterfall.
  private readonly onEvent = (event: ServerEvent): void => {
    if (event.type === "StreamStarted") {
      const { stream_id, device_set, stream } = event.data;
      this.ids.set(stream_id, laneKey(device_set, stream ?? 0));
    } else if (event.type === "StreamStopped" && event.data.kind === "spectrum") {
      this.ids.delete(event.data.stream_id);
    }
  };

  // A reconnect starts with no subscriptions at all, so every lane still being watched has to ask
  // again — otherwise a dropped socket leaves every scope permanently blank. The ids from the old
  // connection are meaningless on the new one.
  private readonly onStatus = (connected: boolean): void => {
    if (!connected) {
      return;
    }
    this.ids.clear();
    for (const lane of this.watched()) {
      this.send(lane, true);
    }
  };

  /** Take over the socket's spectrum frames. Idempotent; attaching a second socket detaches the
   * first. */
  attach(socket: SpectrumSocket): void {
    if (this.socket === socket) {
      return;
    }
    this.detach();
    this.socket = socket;
    socket.addSpectrumListener(this.onFrame);
    socket.addStatusListener(this.onStatus);
    socket.addEventListener(this.onEvent);
    this.ids.clear();
    for (const lane of this.watched()) {
      this.send(lane, true);
    }
  }

  detach(): void {
    const socket = this.socket;
    this.socket = null;
    if (socket === null) {
      return;
    }
    socket.removeSpectrumListener(this.onFrame);
    socket.removeStatusListener(this.onStatus);
    socket.removeEventListener(this.onEvent);
  }

  /** Watch one lane's spectrum. Returns the unsubscribe; the stream stops a grace period after
   * the last watcher of that lane lets go, leaving every other lane of the same radio running. */
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
      // Inside the grace the stream never stopped, so this is a cancelled stop and not a second
      // subscribe: sending one would have the server replace the stream that is already feeding us.
      clearTimeout(lane.release);
      lane.release = 0;
    }
    lane.listeners.add(listener);
    return () => this.release(key, { deviceSet, stream }, listener);
  }

  /** The waterfall a face that has just mounted opens on. Empty for a lane the hub has never
   * carried, or one released long enough ago that its rows were dropped. Copies the ring, so it
   * is a mount-time call and not a per-frame one. */
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

  /** The lane's most recent frame — what a mounting face draws its trace and its readout from
   * before one of its own arrives. */
  latest(deviceSet: number, stream: number): SpectrumFrame | null {
    return this.lanes.get(laneKey(deviceSet, stream))?.history.latest ?? null;
  }

  /** Lanes the server is streaming: every watched one, and any still inside its release grace.
   * The test seam, and what a reconnect re-sends. */
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
    // The history goes with the stream, not with the last face: a lane nobody has watched for
    // this long would otherwise come back showing rows from before the radio was left alone.
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

/** The hub the shell attaches to its socket, module-level like the audio engine so a face
 * remounting never drops a stream another face is still watching. */
export const spectrumHub = new SpectrumHub();
