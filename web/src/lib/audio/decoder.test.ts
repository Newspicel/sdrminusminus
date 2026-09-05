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

beforeEach(() => {
  vi.resetModules();
  FakeWorker.instances = [];
  vi.stubGlobal("AudioDecoder", undefined);
  vi.stubGlobal("Worker", FakeWorker);
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
