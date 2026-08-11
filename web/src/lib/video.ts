// One video subscription per (device set, channel), however many faces watch it.
//
// The same shape as `spectrum.ts` and for the same three reasons: two faces on one channel must
// not each send a `SubscribeVideo` (the server answers the second by replacing the first's
// stream, and either unmount would stop the other's feed); subscriptions are per-connection, so a
// reconnect has to re-send everything still wanted; and a face is remounted by things that have
// nothing to do with its radio — switching between the patch and the rack is the everyday one.
//
// What it does *not* keep is a history. A raster is redrawn fifty times a second and a picture is
// only ever the newest one, so one frame is held per channel — enough that a face which has just
// mounted shows the last picture instead of an empty canvas while it waits for the next.

import type { VideoFrame } from "./frame";
import type { ClientCommand, ServerEvent } from "./types";

/** What the hub needs of a socket — structural, so the unit tests need no WebSocket. */
export interface VideoSocket {
  send(command: ClientCommand): void;
  addVideoListener(listener: (frame: VideoFrame) => void): void;
  removeVideoListener(listener: (frame: VideoFrame) => void): void;
  addStatusListener(listener: (connected: boolean) => void): void;
  removeStatusListener(listener: (connected: boolean) => void): void;
  addEventListener(listener: (event: ServerEvent) => void): void;
  removeEventListener(listener: (event: ServerEvent) => void): void;
}

/** How long a channel's stream outlives its last watcher. A view switch unmounts every face and
 * mounts it again in the same commit; stopping the server's stream on the way through would cost
 * the receiver its sync lock, which the operator reads as a picture that fell apart. */
const RELEASE_GRACE_MS = 5_000;

/** One watched channel. */
export interface VideoChannel {
  deviceSet: number;
  channel: number;
}

/** Map key for a channel. Channel ids are allocated per device set, so two sets both have a
 * channel 1 and the id alone would pour one set's pictures into the other's face. */
function channelKey(deviceSet: number, channel: number): string {
  return `${deviceSet}:${channel}`;
}

interface Watched {
  listeners: Set<(frame: VideoFrame) => void>;
  latest: VideoFrame | null;
  /** A pending stop, or 0 — non-zero is precisely "subscribed, but nothing is watching". */
  release: number;
}

export class VideoHub {
  private socket: VideoSocket | null = null;
  private readonly channels = new Map<string, Watched>();
  /** Server-allocated stream id → channel key, from `VideoStreamStarted`. Frames carry only the
   * id, so losing this mapping silently blanks every picture. */
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

  // A reconnect starts with no subscriptions at all, so every channel still being watched has to
  // ask again. The ids from the old connection are meaningless on the new one.
  private readonly onStatus = (connected: boolean): void => {
    if (!connected) {
      return;
    }
    this.ids.clear();
    for (const watched of this.watched()) {
      this.send(watched, true);
    }
  };

  /** Take over the socket's video frames. Idempotent; attaching a second socket detaches the
   * first. */
  attach(socket: VideoSocket): void {
    if (this.socket === socket) {
      return;
    }
    this.detach();
    this.socket = socket;
    socket.addVideoListener(this.onFrame);
    socket.addStatusListener(this.onStatus);
    socket.addEventListener(this.onEvent);
    this.ids.clear();
    for (const watched of this.watched()) {
      this.send(watched, true);
    }
  }

  detach(): void {
    const socket = this.socket;
    this.socket = null;
    if (socket === null) {
      return;
    }
    socket.removeVideoListener(this.onFrame);
    socket.removeStatusListener(this.onStatus);
    socket.removeEventListener(this.onEvent);
  }

  /** Watch one channel's pictures. Returns the unsubscribe; the stream stops a grace period after
   * the last watcher lets go. */
  subscribe(deviceSet: number, channel: number, listener: (frame: VideoFrame) => void): () => void {
    const key = channelKey(deviceSet, channel);
    let watched = this.channels.get(key);
    if (watched === undefined) {
      watched = { listeners: new Set(), latest: null, release: 0 };
      this.channels.set(key, watched);
      this.send({ deviceSet, channel }, true);
    } else if (watched.release !== 0) {
      // Inside the grace the stream never stopped, so this is a cancelled stop and not a second
      // subscribe: sending one would have the server replace the stream already feeding us.
      clearTimeout(watched.release);
      watched.release = 0;
    }
    watched.listeners.add(listener);
    return () => this.release(key, { deviceSet, channel }, listener);
  }

  /** The channel's most recent picture — what a mounting face draws before one of its own
   * arrives. */
  latest(deviceSet: number, channel: number): VideoFrame | null {
    return this.channels.get(channelKey(deviceSet, channel))?.latest ?? null;
  }

  /** Channels the server is streaming: every watched one, and any still inside its release
   * grace. The test seam, and what a reconnect re-sends. */
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
    // A `send` while the socket is closed is dropped by design; the reconnect path re-sends.
    this.socket?.send({
      type: on ? "SubscribeVideo" : "UnsubscribeVideo",
      data: { device_set: channel.deviceSet, channel: channel.channel },
    });
  }
}

/** The hub the shell attaches to its socket, module-level like the spectrum one so a face
 * remounting never drops a stream another face is still watching. */
export const videoHub = new VideoHub();
