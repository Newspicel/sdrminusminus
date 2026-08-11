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

/** Frame rate and bin count asked of the server. Per connection, not per face (PLAN §9): the
 * server clamps both, and one stream feeds every face watching that lane. */
export const SPECTRUM_FPS = 30;
export const SPECTRUM_BINS = 1024;

/** One watched lane. */
export interface Lane {
  deviceSet: number;
  stream: number;
}

/** Map key for a lane. The pair is the identity; the id the server allocates is not, because it
 * changes on every resubscribe and is absent until one is answered. */
function laneKey(deviceSet: number, stream: number): string {
  return `${deviceSet}:${stream}`;
}

export class SpectrumHub {
  private socket: SpectrumSocket | null = null;
  private readonly watchers = new Map<string, Set<(frame: SpectrumFrame) => void>>();
  /** Server-allocated stream id → lane key, from `StreamStarted`. Frames carry only the id. */
  private readonly ids = new Map<number, string>();

  private readonly onFrame = (frame: SpectrumFrame): void => {
    const key = this.ids.get(frame.streamId);
    if (key === undefined) {
      return;
    }
    const listeners = this.watchers.get(key);
    if (listeners === undefined) {
      return;
    }
    for (const listener of listeners) {
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

  /** Watch one lane's spectrum. Returns the unsubscribe; the stream stops when the last watcher
   * of that lane lets go, leaving every other lane of the same radio running. */
  subscribe(
    deviceSet: number,
    stream: number,
    listener: (frame: SpectrumFrame) => void,
  ): () => void {
    const key = laneKey(deviceSet, stream);
    let listeners = this.watchers.get(key);
    if (listeners === undefined) {
      listeners = new Set();
      this.watchers.set(key, listeners);
      this.send({ deviceSet, stream }, true);
    }
    listeners.add(listener);
    return () => {
      const current = this.watchers.get(key);
      if (current === undefined) {
        return;
      }
      current.delete(listener);
      if (current.size === 0) {
        this.watchers.delete(key);
        this.send({ deviceSet, stream }, false);
      }
    };
  }

  /** Lanes with at least one watcher — the test seam, and what a reconnect re-sends. */
  watched(): Lane[] {
    return [...this.watchers.keys()].map((key) => {
      const [deviceSet, stream] = key.split(":");
      return { deviceSet: Number(deviceSet), stream: Number(stream) };
    });
  }

  private send(lane: Lane, on: boolean): void {
    // A `send` while the socket is closed is dropped by design; the reconnect path re-sends.
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
