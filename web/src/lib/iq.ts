import type { IqFrame } from "./frame";
import type { ClientCommand, ServerEvent } from "./types";

export interface IqSocket {
  send(command: ClientCommand): void;
  addIqListener(listener: (frame: IqFrame) => void): void;
  removeIqListener(listener: (frame: IqFrame) => void): void;
  addStatusListener(listener: (connected: boolean) => void): void;
  removeStatusListener(listener: (connected: boolean) => void): void;
  addEventListener(listener: (event: ServerEvent) => void): void;
  removeEventListener(listener: (event: ServerEvent) => void): void;
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
  listeners: Set<(frame: IqFrame) => void>;
  latest: IqFrame | null;
  release: number;
}

export class IqHub {
  private socket: IqSocket | null = null;
  private readonly taps = new Map<string, Watched>();
  private readonly ids = new Map<number, string>();

  private readonly onFrame = (frame: IqFrame): void => {
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
    if (event.type === "IqStreamStarted") {
      const { stream_id, device_set, channel } = event.data;
      this.ids.set(stream_id, tapKey(device_set, channel));
    } else if (event.type === "StreamStopped" && event.data.kind === "iq") {
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

  attach(socket: IqSocket): void {
    if (this.socket === socket) {
      return;
    }
    this.detach();
    this.socket = socket;
    socket.addIqListener(this.onFrame);
    socket.addStatusListener(this.onStatus);
    socket.addEventListener(this.onEvent);
    this.ids.clear();
    for (const tap of this.watched()) {
      this.send(tap, true);
    }
  }

  detach(): void {
    const socket = this.socket;
    this.socket = null;
    if (socket === null) {
      return;
    }
    socket.removeIqListener(this.onFrame);
    socket.removeStatusListener(this.onStatus);
    socket.removeEventListener(this.onEvent);
  }

  subscribe(deviceSet: number, channel: number, listener: (frame: IqFrame) => void): () => void {
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

  latest(deviceSet: number, channel: number): IqFrame | null {
    return this.taps.get(tapKey(deviceSet, channel))?.latest ?? null;
  }

  watched(): Tap[] {
    return [...this.taps.keys()].map((key) => {
      const [deviceSet, channel] = key.split(":");
      return { deviceSet: Number(deviceSet), channel: Number(channel) };
    });
  }

  private release(key: string, tap: Tap, listener: (frame: IqFrame) => void): void {
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
      type: on ? "SubscribeIq" : "UnsubscribeIq",
      data: { device_set: tap.deviceSet, channel: tap.channel },
    });
  }
}

export const iqHub = new IqHub();
