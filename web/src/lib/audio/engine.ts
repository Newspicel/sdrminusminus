import type { AudioFrame } from "../frame";
import type { Listener, Unsubscribe } from "../listeners";
import type { ClientCommand, ServerEvent } from "../types";
import { LossTracker } from "./loss";
import { monitorKey } from "./monitor";
import { MAX_GAP_FRAMES, SAMPLE_RATE, type WorkletReport } from "./worklet";

export interface AudioSink {
  readonly ready?: Promise<void>;
  push(opus: Uint8Array, timestampUs: number, channels: number): boolean;
  conceal(frames: number): void;
  setVolume(volume: number): void;
  reset(): void;
  close(): void;
}

export type SinkFactory = (
  key: string,
  volume: number,
  onError: (err: unknown) => void,
  onReport: (report: WorkletReport) => void,
) => Promise<AudioSink>;

export interface AudioSocket {
  send(command: ClientCommand): void;
  isConnected(): boolean;
  on<K extends "audio" | "status" | "event">(kind: K, listener: Listener<K>): Unsubscribe;
}

interface ChannelEntry {
  readonly deviceSet: number;
  readonly channel: number;
  readonly fx: readonly string[];
  desired: boolean;
  requested: boolean;
  streamId: number | null;
  sink: AudioSink | null;
  sinkReady: boolean;
  sinkPending: boolean;
  generation: number;
  volume: number;
  lastError: string | null;
  loss: LossTracker;
  lost: number;
  underruns: number;
  sinkUnderruns: number;
  decoderDroppedFrames: number;
  bufferedFrames: number;
  trimmedFrames: number;
}

const US_PER_FRAME = 1_000_000 / SAMPLE_RATE;

const NO_FX: readonly string[] = [];

function entryKey(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): string {
  return monitorKey(deviceSet, channel, fx);
}

function keyOf(entry: ChannelEntry): string {
  return entryKey(entry.deviceSet, entry.channel, entry.fx);
}

function routeData(entry: ChannelEntry): { device_set: number; channel: number; fx?: string[] } {
  const bare = { device_set: entry.deviceSet, channel: entry.channel };
  return entry.fx.length === 0 ? bare : { ...bare, fx: [...entry.fx] };
}

export class AudioEngine {
  private readonly entries = new Map<string, ChannelEntry>();
  private readonly storeListeners = new Set<() => void>();
  private readonly pendingSubscribes: { key: string; generation: number }[] = [];
  private outputRunning = true;
  private socket: AudioSocket | null = null;
  private unsubscribes: Unsubscribe[] = [];

  constructor(private readonly createSink: SinkFactory) {}

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
    this.unsubscribes = [
      socket.on("event", this.handleEvent),
      socket.on("status", this.handleStatus),
      socket.on("audio", this.handleAudio),
    ];
  }

  detach(): void {
    if (!this.socket) {
      return;
    }
    for (const unsubscribe of this.unsubscribes) {
      unsubscribe();
    }
    this.unsubscribes = [];
    this.socket = null;
    for (const entry of this.entries.values()) {
      entry.requested = false;
    }
    this.pendingSubscribes.length = 0;
    this.dropLiveStreams();
  }

  isPlaying(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): boolean {
    const entry = this.entries.get(entryKey(deviceSet, channel, fx));
    return entry !== undefined && entry.desired && entry.streamId !== null && this.outputRunning;
  }

  isPending(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): boolean {
    const entry = this.entries.get(entryKey(deviceSet, channel, fx));
    return entry !== undefined && entry.desired && entry.streamId === null && this.outputRunning;
  }

  isSuspended(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): boolean {
    const entry = this.entries.get(entryKey(deviceSet, channel, fx));
    return entry !== undefined && entry.desired && !this.outputRunning;
  }

  getError(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): string | null {
    return this.entries.get(entryKey(deviceSet, channel, fx))?.lastError ?? null;
  }

  clearError(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): void {
    const entry = this.entries.get(entryKey(deviceSet, channel, fx));
    if (entry && entry.lastError !== null) {
      entry.lastError = null;
      this.notify();
    }
  }

  getVolume(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): number {
    return this.entries.get(entryKey(deviceSet, channel, fx))?.volume ?? 1;
  }

  getLostFrames(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): number {
    return this.entries.get(entryKey(deviceSet, channel, fx))?.lost ?? 0;
  }

  getUnderruns(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): number {
    return this.entries.get(entryKey(deviceSet, channel, fx))?.underruns ?? 0;
  }

  setOutputRunning(running: boolean): void {
    if (this.outputRunning === running) {
      return;
    }
    this.outputRunning = running;
    for (const entry of this.entries.values()) {
      if (!entry.desired) continue;
      if (running) {
        if (!entry.requested) this.ensureSink(entry);
      } else if (entry.requested || entry.sinkReady) {
        entry.generation += 1;
        this.releaseSubscription(entry);
        this.teardown(entry);
      }
    }
    this.notify();
  }

  rerouteOutput(): void {
    for (const entry of this.entries.values()) {
      if (!entry.desired) continue;
      entry.generation += 1;
      this.releaseSubscription(entry);
      this.teardown(entry);
      if (this.outputRunning) this.ensureSink(entry);
    }
    this.notify();
  }

  claimServerError(message: string): boolean {
    const pending = this.pendingSubscribes.shift();
    if (!pending) {
      return false;
    }
    const entry = this.entries.get(pending.key);
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

  start(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): void {
    const entry = this.ensureEntry(deviceSet, channel, fx);
    if (entry.desired) {
      return;
    }
    entry.desired = true;
    entry.lastError = null;
    entry.generation += 1;
    this.notify();
    this.ensureSink(entry);
  }

  stop(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): void {
    const entry = this.entries.get(entryKey(deviceSet, channel, fx));
    if (!entry || !entry.desired) {
      return;
    }
    entry.desired = false;
    entry.generation += 1;
    this.releaseSubscription(entry);
    this.teardown(entry);
    this.notify();
  }

  private releaseSubscription(entry: ChannelEntry): void {
    entry.streamId = null;
    if (entry.requested) {
      entry.requested = false;
      this.socket?.send({
        type: "UnsubscribeAudio",
        data: routeData(entry),
      });
    }
  }

  retain(live: Iterable<{ deviceSet: number; channel: number; fx?: readonly string[] }>): void {
    const keep = new Set<string>();
    for (const { deviceSet, channel, fx } of live) {
      keep.add(entryKey(deviceSet, channel, fx));
    }
    for (const [key, entry] of this.entries) {
      if (entry.desired && !keep.has(key)) {
        this.stop(entry.deviceSet, entry.channel, entry.fx);
      }
    }
  }

  setVolume(
    deviceSet: number,
    channel: number,
    volume: number,
    fx: readonly string[] = NO_FX,
  ): void {
    const entry = this.ensureEntry(deviceSet, channel, fx);
    entry.volume = Math.min(1, Math.max(0, volume));
    entry.sink?.setVolume(entry.volume);
    this.notify();
  }

  private readonly handleEvent = (event: ServerEvent): void => {
    switch (event.type) {
      case "AudioStreamStarted": {
        const key = entryKey(event.data.device_set, event.data.channel, event.data.fx ?? NO_FX);
        const at = this.pendingSubscribes.findIndex((p) => p.key === key);
        const pending = at >= 0 ? this.pendingSubscribes.splice(at, 1)[0] : undefined;
        const entry = this.entries.get(key);
        if (!entry || !entry.desired) {
          break;
        }
        if (pending !== undefined && pending.generation !== entry.generation) {
          break;
        }
        entry.streamId = event.data.stream_id;
        entry.sink?.reset();
        entry.loss.reset();
        this.notify();
        break;
      }
      case "StreamStopped": {
        if (event.data.kind !== "audio") {
          break;
        }
        const entry = this.findByStream(event.data.stream_id);
        if (!entry) {
          break;
        }
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
        if (entry.desired && this.outputRunning) {
          this.ensureSink(entry);
        }
      }
    } else {
      for (const entry of this.entries.values()) {
        entry.requested = false;
      }
      this.pendingSubscribes.length = 0;
      this.dropLiveStreams();
    }
  };

  private readonly handleAudio = (frame: AudioFrame): void => {
    const entry = this.findByStream(frame.streamId);
    const sink = entry?.sink;
    if (!entry || !sink) {
      return;
    }
    const action = entry.loss.next(frame.timestamp);
    if (action.kind !== "continuous") {
      if (action.kind === "reset") {
        sink.reset();
      } else {
        sink.conceal(action.frames);
      }
      entry.lost += action.frames;
      this.notify();
    }
    const accepted = sink.push(
      frame.opus,
      Math.round(Number(frame.timestamp) * US_PER_FRAME),
      frame.chLayout,
    );
    if (!accepted) {
      const frames = entry.loss.packetFrames;
      if (frames !== null) {
        sink.conceal(frames);
        entry.lost += frames;
        this.notify();
      }
    }
  };

  private ensureEntry(
    deviceSet: number,
    channel: number,
    fx: readonly string[] = NO_FX,
  ): ChannelEntry {
    const key = entryKey(deviceSet, channel, fx);
    let entry = this.entries.get(key);
    if (!entry) {
      entry = {
        deviceSet,
        channel,
        fx: [...fx],
        desired: false,
        requested: false,
        streamId: null,
        sink: null,
        sinkReady: false,
        sinkPending: false,
        generation: 0,
        volume: 1,
        lastError: null,
        loss: new LossTracker(MAX_GAP_FRAMES),
        lost: 0,
        underruns: 0,
        bufferedFrames: 0,
        trimmedFrames: 0,
        sinkUnderruns: 0,
        decoderDroppedFrames: 0,
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
    this.createSink(
      keyOf(entry),
      entry.volume,
      (err) => {
        if (entry.desired && entry.generation === generation) this.fail(entry, err);
      },
      (report) => {
        if (entry.desired && entry.generation === generation) this.observe(entry, report);
      },
    )
      .then((sink) => {
        entry.sinkPending = false;
        if (!entry.desired) {
          sink.close();
          return;
        }
        if (generation !== entry.generation) {
          sink.close();
          this.ensureSink(entry);
          return;
        }
        entry.sink = sink;
        entry.sinkUnderruns = 0;
        entry.decoderDroppedFrames = 0;
        entry.bufferedFrames = 0;
        entry.trimmedFrames = 0;
        sink.setVolume(entry.volume);
        void Promise.resolve(sink.ready)
          .then(() => {
            if (entry.sink === sink) {
              entry.sinkReady = true;
              this.sendSubscribe(entry);
            }
          })
          .catch((err: unknown) => {
            if (entry.sink === sink) this.fail(entry, err);
          });
      })
      .catch((err: unknown) => {
        entry.sinkPending = false;
        if (entry.generation !== generation) {
          if (entry.desired) this.ensureSink(entry);
          return;
        }
        this.fail(entry, err);
      });
  }

  private sendSubscribe(entry: ChannelEntry): void {
    if (
      !entry.desired ||
      entry.requested ||
      !entry.sinkReady ||
      !this.outputRunning ||
      !this.socket?.isConnected()
    ) {
      return;
    }
    entry.requested = true;
    this.pendingSubscribes.push({
      key: keyOf(entry),
      generation: entry.generation,
    });
    this.socket.send({
      type: "SubscribeAudio",
      data: routeData(entry),
    });
  }

  getBufferedMs(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): number {
    return (
      ((this.entries.get(entryKey(deviceSet, channel, fx))?.bufferedFrames ?? 0) * 1000) /
      SAMPLE_RATE
    );
  }

  getTrimmedMs(deviceSet: number, channel: number, fx: readonly string[] = NO_FX): number {
    return (
      ((this.entries.get(entryKey(deviceSet, channel, fx))?.trimmedFrames ?? 0) * 1000) /
      SAMPLE_RATE
    );
  }

  private observe(entry: ChannelEntry, report: WorkletReport): void {
    const decoderDropped = report.decoderDroppedFrames ?? entry.decoderDroppedFrames;
    entry.lost += Math.max(0, decoderDropped - entry.decoderDroppedFrames);
    entry.decoderDroppedFrames = decoderDropped;
    const since = report.underruns - entry.sinkUnderruns;
    entry.bufferedFrames = report.bufferedFrames ?? entry.bufferedFrames;
    entry.trimmedFrames = report.trimmedFrames ?? entry.trimmedFrames;
    entry.sinkUnderruns = report.underruns;
    entry.underruns += Math.max(0, since);
    this.notify();
  }

  private fail(entry: ChannelEntry, err: unknown): void {
    console.error(`audio ${keyOf(entry)} failed:`, err);
    entry.lastError = err instanceof Error ? err.message : String(err);
    this.stop(entry.deviceSet, entry.channel, entry.fx);
    this.notify();
  }

  private teardown(entry: ChannelEntry): void {
    entry.sink?.close();
    entry.sink = null;
    entry.sinkReady = false;
    entry.bufferedFrames = 0;
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
