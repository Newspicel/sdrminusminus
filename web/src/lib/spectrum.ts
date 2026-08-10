// One spectrum subscription per device set, however many scope faces are watching it.
//
// The canvas can carry several scope nodes, and two of them on one radio must not each send a
// `SubscribeSpectrum` — the server would answer the second by replacing the first's stream, and
// the unmount of either would stop the other's feed. So subscription is refcounted here, in one
// place, and frames fan out to the faces that asked for that device set (frames carry the
// device-set id as their stream id, PLAN §5).
//
// Reconnects are the other reason this is not per-component state: subscriptions are
// per-connection, so everything wanted must be re-sent when the socket comes back.

import type { SpectrumFrame } from "./frame";
import type { ClientCommand } from "./types";

/** What the hub needs of a socket — structural, so the unit tests need no WebSocket. */
export interface SpectrumSocket {
  send(command: ClientCommand): void;
  isConnected(): boolean;
  addSpectrumListener(listener: (frame: SpectrumFrame) => void): void;
  removeSpectrumListener(listener: (frame: SpectrumFrame) => void): void;
  addStatusListener(listener: (connected: boolean) => void): void;
  removeStatusListener(listener: (connected: boolean) => void): void;
}

/** Frame rate and bin count asked of the server. Per connection, not per face (PLAN §9): the
 * server clamps both, and one stream feeds every face watching that radio. */
export const SPECTRUM_FPS = 30;
export const SPECTRUM_BINS = 1024;

export class SpectrumHub {
  private socket: SpectrumSocket | null = null;
  private readonly watchers = new Map<number, Set<(frame: SpectrumFrame) => void>>();

  private readonly onFrame = (frame: SpectrumFrame): void => {
    const listeners = this.watchers.get(frame.streamId);
    if (listeners === undefined) {
      return;
    }
    for (const listener of listeners) {
      listener(frame);
    }
  };

  // A reconnect starts with no subscriptions at all, so every device set still being watched
  // has to ask again — otherwise a dropped socket leaves every scope permanently blank.
  private readonly onStatus = (connected: boolean): void => {
    if (!connected) {
      return;
    }
    for (const deviceSet of this.watchers.keys()) {
      this.send(deviceSet, true);
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
    for (const deviceSet of this.watchers.keys()) {
      this.send(deviceSet, true);
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
  }

  /** Watch one device set's spectrum. Returns the unsubscribe; the stream stops when the last
   * watcher of that set lets go. */
  subscribe(deviceSet: number, listener: (frame: SpectrumFrame) => void): () => void {
    let listeners = this.watchers.get(deviceSet);
    if (listeners === undefined) {
      listeners = new Set();
      this.watchers.set(deviceSet, listeners);
      this.send(deviceSet, true);
    }
    listeners.add(listener);
    return () => {
      const current = this.watchers.get(deviceSet);
      if (current === undefined) {
        return;
      }
      current.delete(listener);
      if (current.size === 0) {
        this.watchers.delete(deviceSet);
        this.send(deviceSet, false);
      }
    };
  }

  /** Device sets with at least one watcher — the test seam, and what a reconnect re-sends. */
  watched(): number[] {
    return [...this.watchers.keys()];
  }

  private send(deviceSet: number, on: boolean): void {
    // A `send` while the socket is closed is dropped by design; the reconnect path re-sends.
    this.socket?.send(
      on
        ? {
            type: "SubscribeSpectrum",
            data: { device_set: deviceSet, fps: SPECTRUM_FPS, bins: SPECTRUM_BINS },
          }
        : { type: "UnsubscribeSpectrum", data: { device_set: deviceSet } },
    );
  }
}

/** The hub the shell attaches to its socket, module-level like the audio engine so a face
 * remounting never drops a stream another face is still watching. */
export const spectrumHub = new SpectrumHub();
