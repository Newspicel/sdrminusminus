// Audio subscription state machine (PLAN §9), one entry per (device set, channel). Pure of
// WebAudio: the sink factory is injected so the whole lifecycle — subscribe, stream-id
// binding, frame routing, reconnect resubscribe, teardown — is unit-testable.
import type { AudioFrame } from "../frame";
import type { ClientCommand, ServerEvent } from "../types";
import { LossTracker } from "./loss";
import { MAX_SAMPLES, SAMPLE_RATE } from "./worklet";

export interface AudioSink {
  push(opus: Uint8Array, timestampUs: number): void;
  /** Insert `samples` of silence for a detected loss gap so depth and timing stay honest. */
  conceal(samples: number): void;
  setVolume(volume: number): void;
  /** Discard buffered/decoder state; called when a fresh stream id binds to this channel. */
  reset(): void;
  close(): void;
}

export type SinkFactory = (volume: number, onError: (err: unknown) => void) => Promise<AudioSink>;

/** The slice of `SdrSocket` the engine needs; a structural interface so tests can fake it. */
export interface AudioSocket {
  send(command: ClientCommand): void;
  isConnected(): boolean;
  addEventListener(listener: (event: ServerEvent) => void): void;
  removeEventListener(listener: (event: ServerEvent) => void): void;
  addStatusListener(listener: (connected: boolean) => void): void;
  removeStatusListener(listener: (connected: boolean) => void): void;
  onAudio: (frame: AudioFrame) => void;
}

interface ChannelEntry {
  readonly deviceSet: number;
  readonly channel: number;
  /** User pressed start and hasn't stopped; survives socket drops so reconnects resubscribe. */
  desired: boolean;
  /** A SubscribeAudio went out on the current connection, so a stop owes an unsubscribe. */
  requested: boolean;
  streamId: number | null;
  sink: AudioSink | null;
  sinkPending: boolean;
  /** Bumped by start/stop so an in-flight sink creation can detect it lost the race. */
  generation: number;
  volume: number;
  /** Why playback stopped without the user asking; shown until dismissed or restarted. */
  lastError: string | null;
  loss: LossTracker;
}

const US_PER_SAMPLE = 1_000_000 / SAMPLE_RATE;

function entryKey(deviceSet: number, channel: number): string {
  return `${deviceSet}:${channel}`;
}

export class AudioEngine {
  private readonly entries = new Map<string, ChannelEntry>();
  private readonly storeListeners = new Set<() => void>();
  /**
   * Subscribes the server has not yet answered, in send order. The wire has no correlation
   * id, but the server answers subscribes in command order, so the oldest pending entry for
   * a channel is the one an `AudioStreamStarted` (or an `Error`) answers. Without this, a
   * Started for a superseded subscribe binds its stale id to the new intent — and the stale
   * StreamStopped that follows then kills the fresh stream.
   */
  private readonly pendingSubscribes: { key: string; generation: number }[] = [];
  /** Reported by the sink layer: false while the platform audio output is suspended. */
  private outputRunning = true;
  private socket: AudioSocket | null = null;

  constructor(private readonly createSink: SinkFactory) {}

  /** For useSyncExternalStore; stable identity. */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.storeListeners.add(listener);
    return () => this.storeListeners.delete(listener);
  };

  attach(socket: AudioSocket): void {
    if (this.socket === socket) {
      return;
    }
    this.detach();
    this.socket = socket;
    socket.addEventListener(this.handleEvent);
    socket.addStatusListener(this.handleStatus);
    socket.onAudio = this.handleAudio;
  }

  detach(): void {
    const socket = this.socket;
    if (!socket) {
      return;
    }
    socket.removeEventListener(this.handleEvent);
    socket.removeStatusListener(this.handleStatus);
    socket.onAudio = () => {};
    this.socket = null;
    for (const entry of this.entries.values()) {
      entry.requested = false;
    }
    this.pendingSubscribes.length = 0;
    this.dropLiveStreams();
  }

  isPlaying(deviceSet: number, channel: number): boolean {
    const entry = this.entries.get(entryKey(deviceSet, channel));
    return entry !== undefined && entry.desired && entry.streamId !== null && this.outputRunning;
  }

  /** Start requested but no stream bound yet (subscribe in flight or socket down). */
  isPending(deviceSet: number, channel: number): boolean {
    const entry = this.entries.get(entryKey(deviceSet, channel));
    return entry !== undefined && entry.desired && entry.streamId === null;
  }

  /** Stream bound but the platform suspended the audio output — no sound is produced. */
  isSuspended(deviceSet: number, channel: number): boolean {
    const entry = this.entries.get(entryKey(deviceSet, channel));
    return entry !== undefined && entry.desired && entry.streamId !== null && !this.outputRunning;
  }

  getError(deviceSet: number, channel: number): string | null {
    return this.entries.get(entryKey(deviceSet, channel))?.lastError ?? null;
  }

  clearError(deviceSet: number, channel: number): void {
    const entry = this.entries.get(entryKey(deviceSet, channel));
    if (entry && entry.lastError !== null) {
      entry.lastError = null;
      this.notify();
    }
  }

  getVolume(deviceSet: number, channel: number): number {
    return this.entries.get(entryKey(deviceSet, channel))?.volume ?? 1;
  }

  /** Reported by the sink layer; while false, bound streams read as suspended, not playing. */
  setOutputRunning(running: boolean): void {
    if (this.outputRunning === running) {
      return;
    }
    this.outputRunning = running;
    this.notify();
  }

  /**
   * `ServerEvent::Error` carries no coordinates, but the server answers each subscribe in
   * command order — so while a SubscribeAudio is outstanding, an Error is its answer: fail
   * that entry (visible, retryable) instead of leaving it desired-but-unbound forever.
   * Returns whether the error was consumed; unclaimed errors belong to the caller.
   */
  claimServerError(message: string): boolean {
    const pending = this.pendingSubscribes.shift();
    if (!pending) {
      return false;
    }
    const entry = this.entries.get(pending.key);
    // A superseded subscribe failing is moot — a fresher one is still awaiting its answer.
    if (entry && entry.generation === pending.generation) {
      entry.lastError = message;
      entry.desired = false;
      entry.requested = false;
      entry.generation += 1;
      entry.streamId = null;
      this.teardown(entry);
      this.notify();
    }
    return true;
  }

  start(deviceSet: number, channel: number): void {
    const entry = this.ensureEntry(deviceSet, channel);
    if (entry.desired) {
      return;
    }
    entry.desired = true;
    entry.lastError = null;
    entry.generation += 1;
    this.notify();
    this.ensureSink(entry);
  }

  stop(deviceSet: number, channel: number): void {
    const entry = this.entries.get(entryKey(deviceSet, channel));
    if (!entry || !entry.desired) {
      return;
    }
    entry.desired = false;
    entry.generation += 1;
    entry.streamId = null;
    if (entry.requested) {
      entry.requested = false;
      this.socket?.send({
        type: "UnsubscribeAudio",
        data: { device_set: entry.deviceSet, channel: entry.channel },
      });
    }
    this.teardown(entry);
    this.notify();
  }

  setVolume(deviceSet: number, channel: number, volume: number): void {
    const entry = this.ensureEntry(deviceSet, channel);
    entry.volume = Math.min(1, Math.max(0, volume));
    entry.sink?.setVolume(entry.volume);
    this.notify();
  }

  private readonly handleEvent = (event: ServerEvent): void => {
    switch (event.type) {
      case "AudioStreamStarted": {
        const key = entryKey(event.data.device_set, event.data.channel);
        const at = this.pendingSubscribes.findIndex((p) => p.key === key);
        const pending = at >= 0 ? this.pendingSubscribes.splice(at, 1)[0] : undefined;
        const entry = this.entries.get(key);
        if (!entry || !entry.desired) {
          break;
        }
        if (pending !== undefined && pending.generation !== entry.generation) {
          // Answers a superseded subscribe: binding its id would let the StreamStopped that
          // follows (for the unsubscribe we already sent) tear down the current intent.
          break;
        }
        entry.streamId = event.data.stream_id;
        // Fresh stream id ⇒ timestamps restart; stale buffered audio must not play first.
        entry.sink?.reset();
        entry.loss.reset();
        this.notify();
        break;
      }
      case "StreamStopped": {
        // Spectrum and audio ids come from disjoint ranges, but only (kind, id) names a
        // stream — a spectrum stop must never tear down an audio entry.
        if (event.data.kind !== "audio") {
          break;
        }
        const entry = this.findByStream(event.data.stream_id);
        if (!entry) {
          break;
        }
        // The server killed the stream (channel/set removed) — clear intent, don't resubscribe.
        entry.desired = false;
        entry.requested = false;
        entry.generation += 1;
        entry.streamId = null;
        this.teardown(entry);
        this.notify();
        break;
      }
      default:
        break;
    }
  };

  private readonly handleStatus = (connected: boolean): void => {
    if (connected) {
      for (const entry of this.entries.values()) {
        if (entry.desired) {
          this.ensureSink(entry);
        }
      }
    } else {
      // Subscriptions are per-connection (PLAN §5); they died with the socket.
      for (const entry of this.entries.values()) {
        entry.requested = false;
      }
      this.pendingSubscribes.length = 0;
      this.dropLiveStreams();
    }
  };

  private readonly handleAudio = (frame: AudioFrame): void => {
    // Frames for unbound ids are expected churn (e.g. just after unsubscribe): drop.
    const entry = this.findByStream(frame.streamId);
    const sink = entry?.sink;
    if (!entry || !sink) {
      return;
    }
    const action = entry.loss.next(frame.timestamp);
    if (action.kind === "reset") {
      sink.reset();
    } else if (action.kind === "gap") {
      sink.conceal(action.samples);
    }
    sink.push(frame.opus, Math.round(Number(frame.timestamp) * US_PER_SAMPLE));
  };

  private ensureEntry(deviceSet: number, channel: number): ChannelEntry {
    const key = entryKey(deviceSet, channel);
    let entry = this.entries.get(key);
    if (!entry) {
      entry = {
        deviceSet,
        channel,
        desired: false,
        requested: false,
        streamId: null,
        sink: null,
        sinkPending: false,
        generation: 0,
        volume: 1,
        lastError: null,
        loss: new LossTracker(MAX_SAMPLES),
      };
      this.entries.set(key, entry);
    }
    return entry;
  }

  private findByStream(streamId: number): ChannelEntry | undefined {
    for (const entry of this.entries.values()) {
      if (entry.streamId === streamId) {
        return entry;
      }
    }
    return undefined;
  }

  private ensureSink(entry: ChannelEntry): void {
    if (entry.sink) {
      this.sendSubscribe(entry);
      return;
    }
    if (entry.sinkPending) {
      return;
    }
    entry.sinkPending = true;
    const generation = entry.generation;
    this.createSink(entry.volume, (err) => this.fail(entry, err))
      .then((sink) => {
        entry.sinkPending = false;
        if (!entry.desired) {
          sink.close();
          return;
        }
        if (generation !== entry.generation) {
          // A stop/start cycle overtook this creation; retry with the current generation.
          sink.close();
          this.ensureSink(entry);
          return;
        }
        entry.sink = sink;
        sink.setVolume(entry.volume);
        this.sendSubscribe(entry);
      })
      .catch((err: unknown) => {
        entry.sinkPending = false;
        this.fail(entry, err);
      });
  }

  private sendSubscribe(entry: ChannelEntry): void {
    if (!entry.desired || !this.socket?.isConnected()) {
      return;
    }
    entry.requested = true;
    this.pendingSubscribes.push({
      key: entryKey(entry.deviceSet, entry.channel),
      generation: entry.generation,
    });
    this.socket.send({
      type: "SubscribeAudio",
      data: { device_set: entry.deviceSet, channel: entry.channel },
    });
  }

  private fail(entry: ChannelEntry, err: unknown): void {
    console.error(`audio ${entry.deviceSet}:${entry.channel} failed:`, err);
    entry.lastError = err instanceof Error ? err.message : String(err);
    this.stop(entry.deviceSet, entry.channel);
    // stop() is a no-op when intent was already cleared; the error must publish regardless.
    this.notify();
  }

  private teardown(entry: ChannelEntry): void {
    entry.sink?.close();
    entry.sink = null;
  }

  private dropLiveStreams(): void {
    let changed = false;
    for (const entry of this.entries.values()) {
      if (entry.streamId !== null) {
        entry.streamId = null;
        changed = true;
      }
    }
    if (changed) {
      this.notify();
    }
  }

  private notify(): void {
    for (const listener of this.storeListeners) {
      listener();
    }
  }
}
