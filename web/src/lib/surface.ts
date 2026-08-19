import type { RangeDopplerFrame } from "./frame";
import type { Listener, Unsubscribe } from "./listeners";
import type { ClientCommand, ServerEvent } from "./types";

export interface SurfaceSocket {
  send(command: ClientCommand): void;
  isConnected(): boolean;
  on<K extends "surface" | "status" | "event">(kind: K, listener: Listener<K>): Unsubscribe;
}

interface Watched {
  listeners: Set<(frame: RangeDopplerFrame) => void>;
  latest: RangeDopplerFrame | null;
}

/// Keeps one range–Doppler subscription per node however many faces are looking at it, and puts
/// them all back after a reconnect.
export class SurfaceHub {
  private socket: SurfaceSocket | null = null;
  private unsubscribes: Unsubscribe[] = [];
  private readonly nodes = new Map<string, Watched>();
  private readonly ids = new Map<number, string>();

  private readonly onFrame = (frame: RangeDopplerFrame): void => {
    const node = this.ids.get(frame.streamId);
    const watched = node === undefined ? undefined : this.nodes.get(node);
    if (watched === undefined) {
      return;
    }
    watched.latest = frame;
    for (const listener of watched.listeners) {
      listener(frame);
    }
  };

  private readonly onEvent = (event: ServerEvent): void => {
    if (event.type === "SurfaceStreamStarted") {
      this.ids.set(event.data.stream_id, event.data.node);
    } else if (event.type === "StreamStopped" && event.data.kind === "range_doppler") {
      this.ids.delete(event.data.stream_id);
    }
  };

  private readonly onStatus = (connected: boolean): void => {
    if (!connected) {
      return;
    }
    this.ids.clear();
    for (const node of this.nodes.keys()) {
      this.send(node, true);
    }
  };

  attach(socket: SurfaceSocket): void {
    if (this.socket === socket) {
      return;
    }
    this.detach();
    this.socket = socket;
    this.unsubscribes = [
      socket.on("surface", this.onFrame),
      socket.on("status", this.onStatus),
      socket.on("event", this.onEvent),
    ];
    this.ids.clear();
    for (const node of this.nodes.keys()) {
      this.send(node, true);
    }
  }

  detach(): void {
    this.socket = null;
    for (const unsubscribe of this.unsubscribes) {
      unsubscribe();
    }
    this.unsubscribes = [];
  }

  subscribe(node: string, listener: (frame: RangeDopplerFrame) => void): () => void {
    let watched = this.nodes.get(node);
    if (watched === undefined) {
      watched = { listeners: new Set([listener]), latest: null };
      this.nodes.set(node, watched);
      this.send(node, true);
    } else {
      watched.listeners.add(listener);
    }
    return () => {
      const current = this.nodes.get(node);
      if (current === undefined) {
        return;
      }
      current.listeners.delete(listener);
      if (current.listeners.size === 0) {
        this.nodes.delete(node);
        this.send(node, false);
      }
    };
  }

  latest(node: string): RangeDopplerFrame | null {
    return this.nodes.get(node)?.latest ?? null;
  }

  watched(): string[] {
    return [...this.nodes.keys()];
  }

  private send(node: string, on: boolean): void {
    if (this.socket === null || !this.socket.isConnected()) {
      return;
    }
    this.socket.send(
      on
        ? { type: "SubscribeSurface", data: { node } }
        : { type: "UnsubscribeSurface", data: { node } },
    );
  }
}

export const surfaceHub = new SurfaceHub();
