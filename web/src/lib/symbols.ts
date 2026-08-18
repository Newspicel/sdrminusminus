import type { SymbolFrame } from "./frame";
import type { Listener, Unsubscribe } from "./listeners";
import type { ClientCommand, ServerEvent } from "./types";

export interface SymbolSocket {
  send(command: ClientCommand): void;
  on<K extends "symbols" | "status" | "event">(kind: K, listener: Listener<K>): Unsubscribe;
}

const RELEASE_GRACE_MS = 5_000;

export interface Tap {
  deviceSet: number;
  channel: number;
}

function tapKey(deviceSet: number, channel: number): string {
  return `${deviceSet}:${channel}`;
}

interface Watched {
  listeners: Set<(frame: SymbolFrame) => void>;
  latest: SymbolFrame | null;
  release: number;
}

export class SymbolHub {
  private socket: SymbolSocket | null = null;
  private unsubscribes: Unsubscribe[] = [];
  private readonly taps = new Map<string, Watched>();
  private readonly ids = new Map<number, string>();

  private readonly onFrame = (frame: SymbolFrame): void => {
    const key = this.ids.get(frame.streamId);
    const tap = key === undefined ? undefined : this.taps.get(key);
    if (tap === undefined) {
      return;
    }
    tap.latest = frame;
    for (const listener of tap.listeners) {
      listener(frame);
    }
  };

  private readonly onEvent = (event: ServerEvent): void => {
    if (event.type === "SymbolStreamStarted") {
      const { stream_id, device_set, channel } = event.data;
      this.ids.set(stream_id, tapKey(device_set, channel));
    } else if (event.type === "StreamStopped" && event.data.kind === "symbols") {
      this.ids.delete(event.data.stream_id);
    }
  };

  private readonly onStatus = (connected: boolean): void => {
    if (!connected) {
      return;
    }
    this.ids.clear();
    for (const tap of this.watched()) {
      this.send(tap, true);
    }
  };

  attach(socket: SymbolSocket): void {
    if (this.socket === socket) {
      return;
    }
    this.detach();
    this.socket = socket;
    this.unsubscribes = [
      socket.on("symbols", this.onFrame),
      socket.on("status", this.onStatus),
      socket.on("event", this.onEvent),
    ];
    this.ids.clear();
    for (const tap of this.watched()) {
      this.send(tap, true);
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
    channel: number,
    listener: (frame: SymbolFrame) => void,
  ): () => void {
    const key = tapKey(deviceSet, channel);
    let tap = this.taps.get(key);
    if (tap === undefined) {
      tap = { listeners: new Set(), latest: null, release: 0 };
      this.taps.set(key, tap);
      this.send({ deviceSet, channel }, true);
    } else if (tap.release !== 0) {
      clearTimeout(tap.release);
      tap.release = 0;
    }
    tap.listeners.add(listener);
    return () => this.release(key, { deviceSet, channel }, listener);
  }

  latest(deviceSet: number, channel: number): SymbolFrame | null {
    return this.taps.get(tapKey(deviceSet, channel))?.latest ?? null;
  }

  watched(): Tap[] {
    return [...this.taps.keys()].map((key) => {
      const [deviceSet, channel] = key.split(":");
      return { deviceSet: Number(deviceSet), channel: Number(channel) };
    });
  }

  private release(key: string, tap: Tap, listener: (frame: SymbolFrame) => void): void {
    const watched = this.taps.get(key);
    if (watched === undefined) {
      return;
    }
    watched.listeners.delete(listener);
    if (watched.listeners.size > 0 || watched.release !== 0) {
      return;
    }
    watched.release = setTimeout(() => {
      this.taps.delete(key);
      this.send(tap, false);
    }, RELEASE_GRACE_MS);
  }

  private send(tap: Tap, on: boolean): void {
    this.socket?.send({
      type: on ? "SubscribeSymbols" : "UnsubscribeSymbols",
      data: { device_set: tap.deviceSet, channel: tap.channel },
    });
  }
}

export const symbolHub = new SymbolHub();
