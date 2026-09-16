import type { VideoFrame } from "./frame";
import type { Listener, Unsubscribe } from "./listeners";
import { mediaLatencyMs } from "./mediaLatency";
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
  pending: { frame: VideoFrame; due: number }[];
  pendingBytes: number;
  timer: number;
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
    const delay = mediaLatencyMs(key ?? "");
    const due = Date.now() + delay;
    watched.pending.push({ frame, due });
    watched.pendingBytes += frame.pixels.byteLength;
    while (watched.pending.length > 32 || watched.pendingBytes > 64 * 1024 * 1024) {
      const old = watched.pending.shift();
      if (old) watched.pendingBytes -= old.frame.pixels.byteLength;
    }
    this.present(watched);
  };

  private present(watched: Watched): void {
    clearTimeout(watched.timer);
    while (watched.pending[0] && watched.pending[0].due <= Date.now()) {
      const next = watched.pending.shift();
      if (!next) break;
      watched.pendingBytes -= next.frame.pixels.byteLength;
      watched.latest = next.frame;
      for (const listener of watched.listeners) listener(next.frame);
    }
    const next = watched.pending[0];
    watched.timer = next
      ? setTimeout(() => this.present(watched), Math.max(0, next.due - Date.now()))
      : 0;
  }

  private clear(watched: Watched): void {
    clearTimeout(watched.timer);
    watched.pending = [];
    watched.pendingBytes = 0;
    watched.timer = 0;
  }

  private readonly onEvent = (event: ServerEvent): void => {
    if (event.type === "VideoStreamStarted") {
      const { stream_id, device_set, channel } = event.data;
      this.ids.set(stream_id, channelKey(device_set, channel));
    } else if (event.type === "StreamStopped" && event.data.kind === "video") {
      const key = this.ids.get(event.data.stream_id);
      const watched = key === undefined ? undefined : this.channels.get(key);
      if (watched) this.clear(watched);
      this.ids.delete(event.data.stream_id);
    }
  };

  private readonly onStatus = (connected: boolean): void => {
    for (const watched of this.channels.values()) this.clear(watched);
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
    for (const watched of this.channels.values()) this.clear(watched);
    for (const unsubscribe of this.unsubscribes) {
      unsubscribe();
    }
    this.unsubscribes = [];
  }

  subscribe(deviceSet: number, channel: number, listener: (frame: VideoFrame) => void): () => void {
    const key = channelKey(deviceSet, channel);
    let watched = this.channels.get(key);
    if (watched === undefined) {
      watched = {
        listeners: new Set(),
        latest: null,
        release: 0,
        pending: [],
        pendingBytes: 0,
        timer: 0,
      };
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
      this.clear(watched);
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
