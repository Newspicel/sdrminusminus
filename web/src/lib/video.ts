import type { VideoFrame } from "./frame";
import type { Listener, Unsubscribe } from "./listeners";
import type { ClientCommand, ServerEvent } from "./types";

export interface VideoSocket {
  send(command: ClientCommand): void;
  on<K extends "video" | "status" | "event">(kind: K, listener: Listener<K>): Unsubscribe;
}

const RELEASE_GRACE_MS = 5_000;

export interface VideoChannel {
  deviceSet: number;
  channel: number;
}

function channelKey(deviceSet: number, channel: number): string {
  return `${deviceSet}:${channel}`;
}

interface Watched {
  listeners: Set<(frame: VideoFrame) => void>;
  latest: VideoFrame | null;
  release: number;
}

export class VideoHub {
  private socket: VideoSocket | null = null;
  private unsubscribes: Unsubscribe[] = [];
  private readonly channels = new Map<string, Watched>();
  private readonly ids = new Map<number, string>();

  private readonly onFrame = (frame: VideoFrame): void => {
    const key = this.ids.get(frame.streamId);
    const watched = key === undefined ? undefined : this.channels.get(key);
    if (watched === undefined) {
      return;
    }
    watched.latest = frame;
    for (const listener of watched.listeners) {
      listener(frame);
    }
  };

  private readonly onEvent = (event: ServerEvent): void => {
    if (event.type === "VideoStreamStarted") {
      const { stream_id, device_set, channel } = event.data;
      this.ids.set(stream_id, channelKey(device_set, channel));
    } else if (event.type === "StreamStopped" && event.data.kind === "video") {
      this.ids.delete(event.data.stream_id);
    }
  };

  private readonly onStatus = (connected: boolean): void => {
    if (!connected) {
      return;
    }
    this.ids.clear();
    for (const watched of this.watched()) {
      this.send(watched, true);
    }
  };

  attach(socket: VideoSocket): void {
    if (this.socket === socket) {
      return;
    }
    this.detach();
    this.socket = socket;
    this.unsubscribes = [
      socket.on("video", this.onFrame),
      socket.on("status", this.onStatus),
      socket.on("event", this.onEvent),
    ];
    this.ids.clear();
    for (const watched of this.watched()) {
      this.send(watched, true);
    }
  }

  detach(): void {
    this.socket = null;
    for (const unsubscribe of this.unsubscribes) {
      unsubscribe();
    }
    this.unsubscribes = [];
  }

  subscribe(deviceSet: number, channel: number, listener: (frame: VideoFrame) => void): () => void {
    const key = channelKey(deviceSet, channel);
    let watched = this.channels.get(key);
    if (watched === undefined) {
      watched = { listeners: new Set(), latest: null, release: 0 };
      this.channels.set(key, watched);
      this.send({ deviceSet, channel }, true);
    } else if (watched.release !== 0) {
      clearTimeout(watched.release);
      watched.release = 0;
    }
    watched.listeners.add(listener);
    return () => this.release(key, { deviceSet, channel }, listener);
  }

  latest(deviceSet: number, channel: number): VideoFrame | null {
    return this.channels.get(channelKey(deviceSet, channel))?.latest ?? null;
  }

  watched(): VideoChannel[] {
    return [...this.channels.keys()].map((key) => {
      const [deviceSet, channel] = key.split(":");
      return { deviceSet: Number(deviceSet), channel: Number(channel) };
    });
  }

  private release(key: string, channel: VideoChannel, listener: (frame: VideoFrame) => void): void {
    const watched = this.channels.get(key);
    if (watched === undefined) {
      return;
    }
    watched.listeners.delete(listener);
    if (watched.listeners.size > 0 || watched.release !== 0) {
      return;
    }
    watched.release = setTimeout(() => {
      this.channels.delete(key);
      this.send(channel, false);
    }, RELEASE_GRACE_MS);
  }

  private send(channel: VideoChannel, on: boolean): void {
    this.socket?.send({
      type: on ? "SubscribeVideo" : "UnsubscribeVideo",
      data: { device_set: channel.deviceSet, channel: channel.channel },
    });
  }
}

export const videoHub = new VideoHub();
