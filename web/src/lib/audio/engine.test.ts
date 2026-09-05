import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AudioFrame } from "../frame";
import {
  type Listener,
  ListenerRegistry,
  type SocketEventKind,
  type Unsubscribe,
} from "../listeners";
import type { ClientCommand, ServerEvent, StreamKind } from "../types";
import { AudioEngine, type AudioSink, type AudioSocket, type SinkFactory } from "./engine";
import type { WorkletReport } from "./worklet";

class FakeSocket implements AudioSocket {
  sent: ClientCommand[] = [];
  private connected = true;
  private readonly registry = new ListenerRegistry();

  send(command: ClientCommand): void {
    this.sent.push(command);
  }
  isConnected(): boolean {
    return this.connected;
  }
  on<K extends SocketEventKind>(kind: K, listener: Listener<K>): Unsubscribe {
    return this.registry.on(kind, listener);
  }

  onAudio(frame: AudioFrame): void {
    this.registry.emit("audio", frame);
  }
  emit(event: ServerEvent): void {
    this.registry.emit("event", event);
  }
  setConnected(connected: boolean): void {
    this.connected = connected;
    this.registry.emit("status", connected);
  }
}

class FakeSink implements AudioSink {
  pushed: { opus: number[]; timestampUs: number; channels: number }[] = [];
  conceals: number[] = [];
  volume: number;
  resets = 0;
  closed = false;
  accept = true;
  report: (report: WorkletReport) => void = () => {};

  constructor(
    volume: number,
    readonly onError: (err: unknown) => void,
  ) {
    this.volume = volume;
  }
  push(opus: Uint8Array, timestampUs: number, channels: number): boolean {
    this.pushed.push({ opus: Array.from(opus), timestampUs, channels });
    return this.accept;
  }
  conceal(frames: number): void {
    this.conceals.push(frames);
  }
  setVolume(volume: number): void {
    this.volume = volume;
  }
  reset(): void {
    this.resets += 1;
  }
  close(): void {
    this.closed = true;
  }
}

function makeFactory(): { factory: SinkFactory; sinks: FakeSink[]; keys: string[] } {
  const sinks: FakeSink[] = [];
  const keys: string[] = [];
  const factory: SinkFactory = (key, volume, onError, onReport) => {
    const sink = new FakeSink(volume, onError);
    sink.report = onReport;
    sinks.push(sink);
    keys.push(key);
    return Promise.resolve(sink);
  };
  return { factory, sinks, keys };
}

function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function started(deviceSet: number, channel: number, streamId: number): ServerEvent {
  return {
    type: "AudioStreamStarted",
    data: { device_set: deviceSet, channel, stream_id: streamId },
  };
}

function stopped(streamId: number, kind: StreamKind = "audio"): ServerEvent {
  return { type: "StreamStopped", data: { stream_id: streamId, kind } };
}

function audioFrame(
  streamId: number,
  timestamp: bigint,
  bytes: number[],
  chLayout = 1,
): AudioFrame {
  return { streamId, seq: 0, timestamp, chLayout, opus: Uint8Array.from(bytes) };
}

describe("AudioEngine", () => {
  let socket: FakeSocket;
  let sinks: FakeSink[];
  let engine: AudioEngine;

  beforeEach(() => {
    socket = new FakeSocket();
    const made = makeFactory();
    sinks = made.sinks;
    engine = new AudioEngine(made.factory);
    engine.attach(socket);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("start subscribes once the sink is ready, and AudioStreamStarted binds the stream id", async () => {
    engine.start(1, 2);
    expect(engine.isPlaying(1, 2)).toBe(false);
    expect(engine.isPending(1, 2)).toBe(true);
    await flush();
    expect(socket.sent).toEqual([{ type: "SubscribeAudio", data: { device_set: 1, channel: 2 } }]);

    socket.emit(started(1, 2, 9));
    expect(engine.isPlaying(1, 2)).toBe(true);
    expect(engine.isPending(1, 2)).toBe(false);
  });

  it("routes frames by stream id, converting 48 kHz sample timestamps to µs", async () => {
    engine.start(1, 2);
    engine.start(1, 3);
    await flush();
    socket.emit(started(1, 2, 10));
    socket.emit(started(1, 3, 11));

    socket.onAudio(audioFrame(10, 48_000n, [1, 2]));
    socket.onAudio(audioFrame(11, 24_000n, [3]));
    socket.onAudio(audioFrame(99, 0n, [4]));

    expect(sinks[0]?.pushed).toEqual([{ opus: [1, 2], timestampUs: 1_000_000, channels: 1 }]);
    expect(sinks[1]?.pushed).toEqual([{ opus: [3], timestampUs: 500_000, channels: 1 }]);
  });

  it("hands each frame's channel layout to the sink without disturbing the clock", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));

    socket.onAudio(audioFrame(10, 0n, [1], 1));
    socket.onAudio(audioFrame(10, 960n, [2], 2));
    socket.onAudio(audioFrame(10, 1_920n, [3], 2));

    expect(sinks[0]?.pushed).toEqual([
      { opus: [1], timestampUs: 0, channels: 1 },
      { opus: [2], timestampUs: 20_000, channels: 2 },
      { opus: [3], timestampUs: 40_000, channels: 2 },
    ]);
    expect(sinks[0]?.conceals).toEqual([]);
    expect(sinks[0]?.resets).toBe(1);
  });

  it("StreamStopped clears intent, closes the sink, and stops routing", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));
    socket.sent = [];

    socket.emit(stopped(10));
    expect(engine.isPlaying(1, 2)).toBe(false);
    expect(sinks[0]?.closed).toBe(true);
    expect(socket.sent).toEqual([]);
    socket.setConnected(false);
    socket.setConnected(true);
    expect(socket.sent).toEqual([]);

    socket.onAudio(audioFrame(10, 0n, [1]));
    expect(sinks[0]?.pushed).toEqual([]);
  });

  it("a spectrum StreamStopped never touches audio entries", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 7));

    socket.emit(stopped(7, "spectrum"));
    expect(engine.isPlaying(1, 2)).toBe(true);
    expect(sinks[0]?.closed).toBe(false);
  });

  it("stop sends UnsubscribeAudio and tears down the sink", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));

    engine.stop(1, 2);
    expect(engine.isPlaying(1, 2)).toBe(false);
    expect(sinks[0]?.closed).toBe(true);
    expect(socket.sent.at(-1)).toEqual({
      type: "UnsubscribeAudio",
      data: { device_set: 1, channel: 2 },
    });
  });

  it("rapid Play→Stop→Play: the superseded subscribe's Started/Stopped leave the new stream intact", async () => {
    engine.start(1, 2);
    await flush();
    engine.stop(1, 2);
    engine.start(1, 2);
    await flush();
    expect(socket.sent).toEqual([
      { type: "SubscribeAudio", data: { device_set: 1, channel: 2 } },
      { type: "UnsubscribeAudio", data: { device_set: 1, channel: 2 } },
      { type: "SubscribeAudio", data: { device_set: 1, channel: 2 } },
    ]);

    socket.emit(started(1, 2, 0x8000));
    expect(engine.isPlaying(1, 2)).toBe(false);
    socket.emit(stopped(0x8000));
    socket.emit(started(1, 2, 0x8001));
    expect(engine.isPlaying(1, 2)).toBe(true);
    expect(sinks[1]?.closed).toBe(false);

    socket.onAudio(audioFrame(0x8001, 0n, [7]));
    expect(sinks[1]?.pushed).toEqual([{ opus: [7], timestampUs: 0, channels: 1 }]);
    expect(socket.sent).toHaveLength(3);
  });

  it("socket close flips playing false, reconnect resubscribes and rebinds a fresh id", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));
    socket.sent = [];
    const resetsAfterFirstBind = sinks[0]?.resets ?? 0;

    socket.setConnected(false);
    expect(engine.isPlaying(1, 2)).toBe(false);

    socket.setConnected(true);
    expect(socket.sent).toEqual([{ type: "SubscribeAudio", data: { device_set: 1, channel: 2 } }]);

    socket.emit(started(1, 2, 20));
    expect(engine.isPlaying(1, 2)).toBe(true);
    expect(sinks[0]?.resets).toBe(resetsAfterFirstBind + 1);
    socket.onAudio(audioFrame(20, 0n, [5]));
    expect(sinks[0]?.pushed).toEqual([{ opus: [5], timestampUs: 0, channels: 1 }]);
  });

  it("start while disconnected subscribes only once connected", async () => {
    socket.setConnected(false);
    engine.start(1, 2);
    await flush();
    expect(socket.sent).toEqual([]);

    socket.setConnected(true);
    expect(socket.sent).toEqual([{ type: "SubscribeAudio", data: { device_set: 1, channel: 2 } }]);
  });

  it("volume clamps, applies to the live sink, and persists across stop/start", async () => {
    engine.start(1, 2);
    await flush();
    engine.setVolume(1, 2, 0.3);
    expect(sinks[0]?.volume).toBe(0.3);
    engine.setVolume(1, 2, 1.7);
    expect(engine.getVolume(1, 2)).toBe(1);
    engine.setVolume(1, 2, 0.3);

    engine.stop(1, 2);
    engine.start(1, 2);
    await flush();
    expect(engine.getVolume(1, 2)).toBe(0.3);
    expect(sinks[1]?.volume).toBe(0.3);
  });

  it("a decode error surfaces to the store and stops the channel", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));

    sinks[0]?.onError(new Error("decode failed"));
    expect(engine.getError(1, 2)).toBe("decode failed");
    expect(engine.isPlaying(1, 2)).toBe(false);
    expect(sinks[0]?.closed).toBe(true);
    expect(socket.sent.at(-1)).toEqual({
      type: "UnsubscribeAudio",
      data: { device_set: 1, channel: 2 },
    });

    engine.clearError(1, 2);
    expect(engine.getError(1, 2)).toBe(null);
  });

  it("a sink factory failure surfaces to the store and clears intent", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const failing = new AudioEngine(() => Promise.reject(new Error("no audio")));
    failing.attach(socket);
    failing.start(1, 2);
    await flush();
    expect(failing.getError(1, 2)).toBe("no audio");
    expect(failing.isPlaying(1, 2)).toBe(false);
    expect(failing.isPending(1, 2)).toBe(false);
    expect(socket.sent).toEqual([]);
    socket.setConnected(false);
    socket.setConnected(true);
    expect(socket.sent).toEqual([]);

    failing.start(1, 2);
    expect(failing.getError(1, 2)).toBe(null);
  });

  it("claimServerError fails the in-flight subscribe: stopped, retryable, with a visible error", async () => {
    engine.start(1, 2);
    await flush();
    expect(engine.isPending(1, 2)).toBe(true);

    expect(engine.claimServerError("channel 2 not found")).toBe(true);
    expect(engine.getError(1, 2)).toBe("channel 2 not found");
    expect(engine.isPending(1, 2)).toBe(false);
    expect(engine.isPlaying(1, 2)).toBe(false);
    expect(sinks[0]?.closed).toBe(true);

    socket.setConnected(false);
    socket.setConnected(true);
    expect(socket.sent.filter((c) => c.type === "SubscribeAudio")).toHaveLength(1);

    engine.start(1, 2);
    expect(engine.getError(1, 2)).toBe(null);
    expect(engine.isPending(1, 2)).toBe(true);
  });

  it("claimServerError without an in-flight subscribe is not claimed", () => {
    expect(engine.claimServerError("invalid command")).toBe(false);
  });

  it("an error answering a superseded subscribe does not kill the replacing one", async () => {
    engine.start(1, 2);
    await flush();
    engine.stop(1, 2);
    engine.start(1, 2);
    await flush();

    expect(engine.claimServerError("too late")).toBe(true);
    expect(engine.isPending(1, 2)).toBe(true);
    expect(engine.getError(1, 2)).toBe(null);

    socket.emit(started(1, 2, 0x8001));
    expect(engine.isPlaying(1, 2)).toBe(true);
  });

  it("a suspended audio output reads as suspended, not playing, until it resumes", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));
    expect(engine.isPlaying(1, 2)).toBe(true);

    engine.setOutputRunning(false);
    expect(engine.isPlaying(1, 2)).toBe(false);
    expect(engine.isSuspended(1, 2)).toBe(true);

    engine.setOutputRunning(true);
    expect(engine.isPlaying(1, 2)).toBe(true);
    expect(engine.isSuspended(1, 2)).toBe(false);
  });

  it("timestamp gaps conceal the missing audio; oversized gaps reset the sink", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));
    const resetsAfterBind = sinks[0]?.resets ?? 0;

    socket.onAudio(audioFrame(10, 0n, [1]));
    socket.onAudio(audioFrame(10, 960n, [2]));
    socket.onAudio(audioFrame(10, 1_920n, [3]));
    expect(sinks[0]?.conceals).toEqual([]);

    socket.onAudio(audioFrame(10, 3_840n, [4]));
    expect(sinks[0]?.conceals).toEqual([960]);

    socket.onAudio(audioFrame(10, 34_800n, [5]));
    expect(sinks[0]?.resets).toBe(resetsAfterBind + 1);
    expect(sinks[0]?.pushed).toHaveLength(5);
  });

  it("keeps stream loss and local starvation apart, and accumulates both across sinks", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));
    expect(engine.getLostFrames(1, 2)).toBe(0);
    expect(engine.getUnderruns(1, 2)).toBe(0);

    socket.onAudio(audioFrame(10, 0n, [1]));
    socket.onAudio(audioFrame(10, 960n, [2]));
    socket.onAudio(audioFrame(10, 2_880n, [3]));
    expect(engine.getLostFrames(1, 2)).toBe(960);
    expect(engine.getUnderruns(1, 2)).toBe(0);

    sinks[0]?.report({ underruns: 2 });
    sinks[0]?.report({ underruns: 3 });
    expect(engine.getUnderruns(1, 2)).toBe(3);
    expect(engine.getLostFrames(1, 2)).toBe(960);

    engine.stop(1, 2);
    engine.start(1, 2);
    await flush();
    sinks[1]?.report({ underruns: 1 });
    expect(engine.getUnderruns(1, 2)).toBe(4);
  });

  it("reports buffer and discarded audio, and clears the buffer when stopped", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));
    sinks[0]?.report({
      underruns: 0,
      bufferedFrames: 2880,
      decoderDroppedFrames: 960,
      trimmedFrames: 480,
    });
    expect(engine.getBufferedMs(1, 2)).toBe(60);
    expect(engine.getTrimmedMs(1, 2)).toBe(10);
    expect(engine.getLostFrames(1, 2)).toBe(960);

    engine.stop(1, 2);
    expect(engine.getBufferedMs(1, 2)).toBe(0);
  });

  it("retain stops the channels the canvas can no longer reach, and only those", async () => {
    engine.start(1, 2);
    engine.start(1, 3);
    await flush();
    socket.emit(started(1, 2, 10));
    socket.emit(started(1, 3, 11));
    socket.sent.length = 0;

    engine.retain([{ deviceSet: 1, channel: 2 }]);

    expect(engine.isPlaying(1, 2)).toBe(true);
    expect(engine.isPlaying(1, 3)).toBe(false);
    expect(engine.isPending(1, 3)).toBe(false);
    expect(socket.sent).toEqual([
      { type: "UnsubscribeAudio", data: { device_set: 1, channel: 3 } },
    ]);
    expect(sinks[1]?.closed).toBe(true);
    expect(sinks[0]?.closed).toBe(false);
  });

  it("retain leaves a stopped channel alone rather than resubscribing or re-erroring", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));
    engine.stop(1, 2);
    socket.sent.length = 0;

    engine.retain([]);
    expect(socket.sent).toEqual([]);
  });

  it("conceals a packet the sink refuses, so the loss cannot pass as an underrun", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));
    const sink = sinks[0];

    socket.onAudio(audioFrame(10, 0n, [1]));
    socket.onAudio(audioFrame(10, 960n, [2]));
    expect(sink?.conceals).toEqual([]);

    if (sink) {
      sink.accept = false;
    }
    socket.onAudio(audioFrame(10, 1_920n, [3]));
    expect(sink?.conceals).toEqual([960]);
  });

  it("a stop/start race during sink creation still ends with a live sink", async () => {
    engine.start(1, 2);
    engine.stop(1, 2);
    engine.start(1, 2);
    await flush();
    expect(sinks[0]?.closed).toBe(true);
    expect(sinks[1]?.closed).toBe(false);
    expect(socket.sent.filter((c) => c.type === "SubscribeAudio")).toEqual([
      { type: "SubscribeAudio", data: { device_set: 1, channel: 2 } },
    ]);
  });

  it("notifies store subscribers on state changes", async () => {
    const listener = vi.fn();
    const unsubscribe = engine.subscribe(listener);
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));
    expect(listener).toHaveBeenCalled();

    listener.mockClear();
    unsubscribe();
    engine.stop(1, 2);
    expect(listener).not.toHaveBeenCalled();
  });
});
