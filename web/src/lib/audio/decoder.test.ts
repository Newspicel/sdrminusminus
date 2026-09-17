import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { DecoderRequest, DecoderResponse } from "./workerProtocol";

class FakeWorker {
  static instances: FakeWorker[] = [];
  onmessage: ((event: MessageEvent<DecoderResponse>) => void) | null = null;
  onerror: ((event: ErrorEvent) => void) | null = null;
  terminate = vi.fn();
  messages: DecoderRequest[] = [];
  constructor() {
    FakeWorker.instances.push(this);
  }
  postMessage(message: DecoderRequest): void {
    this.messages.push(message);
    if (message.type === "init") queueMicrotask(() => this.emit({ type: "ready" }));
  }
  emit(data: DecoderResponse): void {
    this.onmessage?.({ data } as MessageEvent<DecoderResponse>);
  }
}

class FakeChunk {
  readonly timestamp: number;
  constructor(init: { timestamp: number }) {
    this.timestamp = init.timestamp;
  }
}

class FakeAudioDecoder {
  static instances: FakeAudioDecoder[] = [];
  static isConfigSupported = vi.fn(async () => ({ supported: true }));
  state = "unconfigured";
  outputs: number[] = [];
  private base: number | null = null;
  private decoded = 0;
  private readonly output: (data: AudioData) => void;
  constructor(init: { output: (data: AudioData) => void }) {
    this.output = init.output;
    FakeAudioDecoder.instances.push(this);
  }
  configure(): void {
    this.state = "configured";
  }
  decode(chunk: FakeChunk): void {
    this.base ??= chunk.timestamp;
    const timestamp = this.base + Math.round((this.decoded * 1_000_000) / 48_000);
    this.decoded += 960;
    this.outputs.push(timestamp);
    this.output({
      timestamp,
      numberOfFrames: 960,
      copyTo: (plane: Float32Array) => plane.fill(0.5),
      close: () => {},
    } as unknown as AudioData);
  }
  reset(): void {
    this.base = null;
    this.decoded = 0;
    this.state = "unconfigured";
  }
  close(): void {
    this.state = "closed";
  }
}

beforeEach(() => {
  vi.resetModules();
  FakeWorker.instances = [];
  FakeAudioDecoder.instances = [];
  vi.stubGlobal("AudioDecoder", undefined);
  vi.stubGlobal("Worker", FakeWorker);
});

function activeWorker(): FakeWorker {
  const worker = FakeWorker.instances[0];
  if (!worker) throw new Error("worker was not created");
  return worker;
}

it("cleans up a worker when initialization cannot be sent", async () => {
  vi.spyOn(FakeWorker.prototype, "postMessage").mockImplementation(() => {
    throw new Error("initialization failed");
  });
  const { createOpusPacketDecoder } = await import("./decoder");
  const error = vi.fn();
  await expect(createOpusPacketDecoder(1, vi.fn(), error)).rejects.toThrow("initialization failed");
  expect(activeWorker().terminate).toHaveBeenCalledTimes(1);
  expect(error).not.toHaveBeenCalled();
});

it("reports a fatal worker failure after initialization instead of silently losing every packet", async () => {
  const { createOpusPacketDecoder } = await import("./decoder");
  const pcm = vi.fn();
  const error = vi.fn();
  const decoder = await createOpusPacketDecoder(1, pcm, error);
  const worker = activeWorker();
  expect(decoder.decode(new Uint8Array([1]), 0)).toBe(true);
  decoder.reset();
  worker.emit({ type: "error", message: "reset failed" });
  expect(error).toHaveBeenCalledExactlyOnceWith(new Error("reset failed"));
  expect(worker.terminate).toHaveBeenCalledTimes(1);
  expect(decoder.decode(new Uint8Array([1]), 1)).toBe(false);
  worker.emit({ type: "pcm", id: 0, epoch: 0, pcm: new Float32Array(960) });
  expect(pcm).not.toHaveBeenCalled();
  decoder.close();
});

it("ignores a queued worker error after the listener has closed", async () => {
  const { createOpusPacketDecoder } = await import("./decoder");
  const error = vi.fn();
  const decoder = await createOpusPacketDecoder(1, vi.fn(), error);
  const worker = activeWorker();
  decoder.close();
  worker.onerror?.({ message: "late worker error" } as ErrorEvent);
  expect(error).not.toHaveBeenCalled();
});

it("reports rejected worker messages without leaking queue slots", async () => {
  const { createOpusPacketDecoder } = await import("./decoder");
  const error = vi.fn();
  const decoder = await createOpusPacketDecoder(1, vi.fn(), error);
  const worker = activeWorker();
  const post = vi.spyOn(worker, "postMessage").mockImplementation(() => {
    throw new DOMException("could not send", "DataCloneError");
  });
  for (let index = 0; index < 6; index++) {
    expect(decoder.decode(new Uint8Array([1]), index)).toBe(false);
  }
  expect(error).toHaveBeenCalledTimes(6);
  post.mockRestore();
  for (let index = 0; index < 5; index++) {
    expect(decoder.decode(new Uint8Array([1]), index)).toBe(true);
  }
  expect(decoder.decode(new Uint8Array([1]), 6)).toBe(false);
  decoder.close();
});

it("reports rejected native packets without leaking queue slots", async () => {
  vi.stubGlobal("AudioDecoder", FakeAudioDecoder);
  vi.stubGlobal("EncodedAudioChunk", FakeChunk);
  const { createOpusPacketDecoder } = await import("./decoder");
  const error = vi.fn();
  const pcm = vi.fn();
  const decoder = await createOpusPacketDecoder(1, pcm, error);
  const decode = vi.spyOn(FakeAudioDecoder.prototype, "decode").mockImplementation(() => {
    throw new DOMException("bad packet", "DataError");
  });
  for (let index = 0; index < 6; index++) {
    expect(decoder.decode(new Uint8Array([1]), index)).toBe(false);
  }
  expect(error).toHaveBeenCalledTimes(6);
  decode.mockRestore();
  expect(decoder.decode(new Uint8Array([1]), 140_000)).toBe(true);
  expect(pcm).toHaveBeenCalledTimes(1);
  decoder.close();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it("keeps WASM decoding off the main thread with bounded in-flight packets", async () => {
  const { createOpusPacketDecoder } = await import("./decoder");
  const pcm = vi.fn();
  const decoder = await createOpusPacketDecoder(1, pcm, vi.fn());
  const worker = FakeWorker.instances[0];
  for (let index = 0; index < 5; index++)
    expect(decoder.decode(new Uint8Array([1, 2]), index)).toBe(true);
  expect(decoder.decode(new Uint8Array([1, 2]), 6)).toBe(false);
  worker?.emit({ type: "pcm", id: 0, epoch: 0, pcm: new Float32Array([0.5]) });
  expect(pcm).toHaveBeenCalledWith(new Float32Array([0.5]));
  expect(decoder.decode(new Uint8Array([3]), 7)).toBe(true);
  decoder.close();
  expect(worker?.terminate).toHaveBeenCalled();
  expect(decoder.decode(new Uint8Array([4]), 8)).toBe(false);
});

it("discards decoder output from before a reset and releases its queue credits", async () => {
  const { createOpusPacketDecoder } = await import("./decoder");
  const pcm = vi.fn();
  const decoder = await createOpusPacketDecoder(1, pcm, vi.fn());
  const worker = FakeWorker.instances[0];
  decoder.decode(new Uint8Array([1]), 0);
  decoder.reset();
  worker?.emit({ type: "pcm", id: 0, epoch: 0, pcm: new Float32Array([0.5]) });
  expect(pcm).not.toHaveBeenCalled();
  decoder.decode(new Uint8Array([1]), 1);
  worker?.emit({ type: "pcm", id: 1, epoch: 1, pcm: new Float32Array([0.25]) });
  expect(pcm).toHaveBeenCalledWith(new Float32Array([0.25]));
  decoder.close();
});

it("drops late worker results without stopping playback", async () => {
  const { createOpusPacketDecoder } = await import("./decoder");
  const pcm = vi.fn();
  const error = vi.fn();
  const dropped = vi.fn();
  const now = vi.spyOn(performance, "now").mockReturnValue(0);
  const decoder = await createOpusPacketDecoder(1, pcm, error, dropped);
  decoder.decode(new Uint8Array([1]), 0);
  now.mockReturnValue(200);
  FakeWorker.instances[0]?.emit({ type: "pcm", id: 0, epoch: 0, pcm: new Float32Array(960) });
  expect(pcm).not.toHaveBeenCalled();
  expect(error).not.toHaveBeenCalled();
  expect(dropped).toHaveBeenCalledWith(960);
  expect(decoder.decode(new Uint8Array([1]), 1)).toBe(true);
  decoder.close();
});

it("keeps decoding across a timestamp gap on Chromium, which stamps outputs from the first chunk", async () => {
  vi.stubGlobal("AudioDecoder", FakeAudioDecoder);
  vi.stubGlobal("EncodedAudioChunk", FakeChunk);
  const { createOpusPacketDecoder } = await import("./decoder");
  const pcm = vi.fn();
  const dropped = vi.fn();
  const decoder = await createOpusPacketDecoder(1, pcm, vi.fn(), dropped);
  for (const timestamp of [0, 20_000, 60_000, 80_000]) {
    expect(decoder.decode(new Uint8Array([1]), timestamp)).toBe(true);
  }
  expect(FakeAudioDecoder.instances[0]?.outputs).toEqual([0, 20_000, 40_000, 60_000]);
  expect(pcm).toHaveBeenCalledTimes(4);
  expect(dropped).not.toHaveBeenCalled();
  decoder.close();
  expect(decoder.decode(new Uint8Array([1]), 100_000)).toBe(false);
});
