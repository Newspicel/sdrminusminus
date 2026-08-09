import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AudioFrame } from "../frame";
import type { ClientCommand, ServerEvent, StreamKind } from "../types";
import { AudioEngine, type AudioSink, type AudioSocket, type SinkFactory } from "./engine";

class FakeSocket implements AudioSocket {
  sent: ClientCommand[] = [];
  onAudio: (frame: AudioFrame) => void = () => {};
  private connected = true;
  private readonly eventListeners = new Set<(event: ServerEvent) => void>();
  private readonly statusListeners = new Set<(connected: boolean) => void>();

  send(command: ClientCommand): void {
    this.sent.push(command);
  }
  isConnected(): boolean {
    return this.connected;
  }
  addEventListener(listener: (event: ServerEvent) => void): void {
    this.eventListeners.add(listener);
  }
  removeEventListener(listener: (event: ServerEvent) => void): void {
    this.eventListeners.delete(listener);
  }
  addStatusListener(listener: (connected: boolean) => void): void {
    this.statusListeners.add(listener);
  }
  removeStatusListener(listener: (connected: boolean) => void): void {
    this.statusListeners.delete(listener);
  }

  emit(event: ServerEvent): void {
    for (const listener of this.eventListeners) {
      listener(event);
    }
  }
  setConnected(connected: boolean): void {
    this.connected = connected;
    for (const listener of this.statusListeners) {
      listener(connected);
    }
  }
}

class FakeSink implements AudioSink {
  pushed: { opus: number[]; timestampUs: number }[] = [];
  conceals: number[] = [];
  volume: number;
  resets = 0;
  closed = false;

  constructor(
    volume: number,
    readonly onError: (err: unknown) => void,
  ) {
    this.volume = volume;
  }
  push(opus: Uint8Array, timestampUs: number): void {
    this.pushed.push({ opus: Array.from(opus), timestampUs });
  }
  conceal(samples: number): void {
    this.conceals.push(samples);
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

function makeFactory(): { factory: SinkFactory; sinks: FakeSink[] } {
  const sinks: FakeSink[] = [];
  const factory: SinkFactory = (volume, onError) => {
    const sink = new FakeSink(volume, onError);
    sinks.push(sink);
    return Promise.resolve(sink);
  };
  return { factory, sinks };
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

function audioFrame(streamId: number, timestamp: bigint, bytes: number[]): AudioFrame {
  return { streamId, seq: 0, timestamp, chLayout: 1, opus: Uint8Array.from(bytes) };
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

    expect(sinks[0]?.pushed).toEqual([{ opus: [1, 2], timestampUs: 1_000_000 }]);
    expect(sinks[1]?.pushed).toEqual([{ opus: [3], timestampUs: 500_000 }]);
  });

  it("StreamStopped clears intent, closes the sink, and stops routing", async () => {
    engine.start(1, 2);
    await flush();
    socket.emit(started(1, 2, 10));
    socket.sent = [];

    socket.emit(stopped(10));
    expect(engine.isPlaying(1, 2)).toBe(false);
    expect(sinks[0]?.closed).toBe(true);
    // Server-initiated stop: no UnsubscribeAudio and no resubscribe on reconnect.
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

    // The server answers in command order: Started for subscribe #1, Stopped for the
    // unsubscribe, Started for subscribe #2.
    socket.emit(started(1, 2, 0x8000));
    // The stale id must not bind to the new intent.
    expect(engine.isPlaying(1, 2)).toBe(false);
    socket.emit(stopped(0x8000));
    socket.emit(started(1, 2, 0x8001));
    expect(engine.isPlaying(1, 2)).toBe(true);
    expect(sinks[1]?.closed).toBe(false);

    socket.onAudio(audioFrame(0x8001, 0n, [7]));
    expect(sinks[1]?.pushed).toEqual([{ opus: [7], timestampUs: 0 }]);
    // The stale Stopped must not have cancelled the still-owed subscription.
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
    // The rebind resets the sink: stale pre-disconnect audio must not play first.
    expect(sinks[0]?.resets).toBe(resetsAfterFirstBind + 1);
    socket.onAudio(audioFrame(20, 0n, [5]));
    expect(sinks[0]?.pushed).toEqual([{ opus: [5], timestampUs: 0 }]);
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

    // A retry starts clean.
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

    // Intent was cleared: a reconnect must not resubscribe the failed stream.
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

    // Oldest pending answer is subscribe #1's failure; subscribe #2 is still owed a Started.
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

    // One 960-sample packet lost: 1920 → 3840 skips exactly one packet.
    socket.onAudio(audioFrame(10, 3_840n, [4]));
    expect(sinks[0]?.conceals).toEqual([960]);

    // A hole wider than the jitter cap cannot be concealed: drop and rebuffer.
    socket.onAudio(audioFrame(10, 34_800n, [5]));
    expect(sinks[0]?.resets).toBe(resetsAfterBind + 1);
    // Every received frame still reaches the decoder, in order.
    expect(sinks[0]?.pushed).toHaveLength(5);
  });

  it("a stop/start race during sink creation still ends with a live sink", async () => {
    engine.start(1, 2);
    engine.stop(1, 2);
    engine.start(1, 2);
    await flush();
    // The first creation lost the race and was closed; the retry serves the new start.
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
