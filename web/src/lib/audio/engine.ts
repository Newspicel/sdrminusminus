import type { AudioFrame } from "../frame";
import type { ClientCommand, ServerEvent } from "../types";
import { LossTracker } from "./loss";
import { MAX_GAP_FRAMES, SAMPLE_RATE, type WorkletReport } from "./worklet";

export interface AudioSink {
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
  addEventListener(listener: (event: ServerEvent) => void): void;
  removeEventListener(listener: (event: ServerEvent) => void): void;
  addStatusListener(listener: (connected: boolean) => void): void;
  removeStatusListener(listener: (connected: boolean) => void): void;
  onAudio: (frame: AudioFrame) => void;
}

interface ChannelEntry {
  readonly deviceSet: number;
  readonly channel: number;
  desired: boolean;
  requested: boolean;
  streamId: number | null;
  sink: AudioSink | null;
  sinkPending: boolean;
  generation: number;
  volume: number;
  lastError: string | null;
  loss: LossTracker;
  lost: number;
  underruns: number;
  sinkUnderruns: number;
}

const US_PER_FRAME = 1_000_000 / SAMPLE_RATE;

function entryKey(deviceSet: number, channel: number): string {
  return `${deviceSet}:${channel}`;
}

export class AudioEngine {
  private readonly entries = new Map<string, ChannelEntry>();
  private readonly storeListeners = new Set<() => void>();
  private readonly pendingSubscribes: { key: string; generation: number }[] = [];
  private outputRunning = true;
  private socket: AudioSocket | null = null;

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

  isPending(deviceSet: number, channel: number): boolean {
    const entry = this.entries.get(entryKey(deviceSet, channel));
    return entry !== undefined && entry.desired && entry.streamId === null;
  }

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

  getLostFrames(deviceSet: number, channel: number): number {
    return this.entries.get(entryKey(deviceSet, channel))?.lost ?? 0;
  }

  getUnderruns(deviceSet: number, channel: number): number {
    return this.entries.get(entryKey(deviceSet, channel))?.underruns ?? 0;
  }

  setOutputRunning(running: boolean): void {
    if (this.outputRunning === running) {
      return;
    }
    this.outputRunning = running;
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

  retain(live: Iterable<{ deviceSet: number; channel: number }>): void {
    const keep = new Set<string>();
    for (const { deviceSet, channel } of live) {
      keep.add(entryKey(deviceSet, channel));
    }
    for (const [key, entry] of this.entries) {
      if (entry.desired && !keep.has(key)) {
        this.stop(entry.deviceSet, entry.channel);
      }
    }
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
        if (entry.desired) {
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
        loss: new LossTracker(MAX_GAP_FRAMES),
        lost: 0,
        underruns: 0,
        sinkUnderruns: 0,
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
      entryKey(entry.deviceSet, entry.channel),
      entry.volume,
      (err) => this.fail(entry, err),
      (report) => this.observe(entry, report),
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

  private observe(entry: ChannelEntry, report: WorkletReport): void {
    const since = report.underruns - entry.sinkUnderruns;
    if (since <= 0) {
      return;
    }
    entry.sinkUnderruns = report.underruns;
    entry.underruns += since;
    this.notify();
  }

  private fail(entry: ChannelEntry, err: unknown): void {
    console.error(`audio ${entry.deviceSet}:${entry.channel} failed:`, err);
    entry.lastError = err instanceof Error ? err.message : String(err);
    this.stop(entry.deviceSet, entry.channel);
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
