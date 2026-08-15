// One baseband subscription per (device set, channel), however many faces watch it.
//
// The same shape as the spectrum hub, and for the same three reasons: two scopes on one channel
// must cost one stream and neither may stop the other's; a reconnect starts with no subscriptions
// at all, so everything wanted has to be re-sent; and a face remounts for reasons that have
// nothing to do with its channel, so the last burst outlives any one of them.
//
// Frames carry only a stream id the server allocates per connection, so the id is learned from the
// `IqStreamStarted` that answers each subscribe and forgotten on `StreamStopped`.
//
// Unlike the spectrum hub there is no history ring: the tap sends bursts of consecutive samples
// with gaps between them, so a "recent past" made of several bursts would be a stream that never
// existed. Only the newest burst is kept.

import type { IqFrame } from "./frame";
import type { ClientCommand, ServerEvent } from "./types";

/** What the hub needs of a socket — structural, so the unit tests need no WebSocket. */
export interface IqSocket {
  send(command: ClientCommand): void;
  addIqListener(listener: (frame: IqFrame) => void): void;
  removeIqListener(listener: (frame: IqFrame) => void): void;
  addStatusListener(listener: (connected: boolean) => void): void;
  removeStatusListener(listener: (connected: boolean) => void): void;
  addEventListener(listener: (event: ServerEvent) => void): void;
  removeEventListener(listener: (event: ServerEvent) => void): void;
}

/** How long a channel's stream outlives its last watcher. A view switch unmounts every face and
 * mounts it again in the same commit; stopping the tap on the way through would cost a restart. */
const RELEASE_GRACE_MS = 5_000;

/** One watched channel. */
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
  /** A pending stop, or 0 — non-zero is precisely "subscribed, but nothing is watching". */
  release: number;
}

export class IqHub {
  private socket: IqSocket | null = null;
  private readonly taps = new Map<string, Watched>();
  /** Server-allocated stream id → tap key, from `IqStreamStarted`. Frames carry only the id. */
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
    // The ids from the old connection are meaningless on the new one.
    this.ids.clear();
    for (const tap of this.watched()) {
      this.send(tap, true);
    }
  };

  /** Take over the socket's IQ frames. Idempotent; attaching a second socket detaches the first. */
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

  /** Watch one channel's baseband. Returns the unsubscribe; the tap stops a grace period after
   * the last watcher lets go. */
  subscribe(deviceSet: number, channel: number, listener: (frame: IqFrame) => void): () => void {
    const key = tapKey(deviceSet, channel);
    let tap = this.taps.get(key);
    if (tap === undefined) {
      tap = { listeners: new Set(), latest: null, release: 0 };
      this.taps.set(key, tap);
      this.send({ deviceSet, channel }, true);
    } else if (tap.release !== 0) {
      // Inside the grace the stream never stopped, so this is a cancelled stop and not a second
      // subscribe: sending one would have the server replace the stream already feeding us.
      clearTimeout(tap.release);
      tap.release = 0;
    }
    tap.listeners.add(listener);
    return () => this.release(key, { deviceSet, channel }, listener);
  }

  /** The newest burst, which is what a mounting face draws before one of its own arrives. */
  latest(deviceSet: number, channel: number): IqFrame | null {
    return this.taps.get(tapKey(deviceSet, channel))?.latest ?? null;
  }

  /** Channels the server is streaming: every watched one, and any still inside its release
   * grace. The test seam, and what a reconnect re-sends. */
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

/** The hub the shell attaches to its socket, module-level like the spectrum one so a face
 * remounting never drops a tap another face is still watching. */
export const iqHub = new IqHub();
