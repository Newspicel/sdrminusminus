// The sink factory owns real WebAudio resources; these tests fake the WebAudio globals to
// pin the lifecycle contracts jsdom can't exercise: failure-path teardown and the
// suspended-context health reporting.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { createDecoder } = vi.hoisted(() => ({ createDecoder: vi.fn() }));
vi.mock("./decoder", () => ({ createOpusPacketDecoder: createDecoder }));

class FakeAudioContext {
  static initialState = "running";
  static instances: FakeAudioContext[] = [];

  state = FakeAudioContext.initialState;
  destination = {};
  audioWorklet = { addModule: vi.fn(() => Promise.resolve()) };
  resume = vi.fn(() => Promise.resolve());
  private readonly listeners = new Set<() => void>();

  constructor() {
    FakeAudioContext.instances.push(this);
  }
  addEventListener(_type: string, listener: () => void): void {
    this.listeners.add(listener);
  }
  removeEventListener(_type: string, listener: () => void): void {
    this.listeners.delete(listener);
  }
  setState(state: string): void {
    this.state = state;
    for (const listener of this.listeners) {
      listener();
    }
  }
}

class FakeWorkletNode {
  port = { postMessage: vi.fn() };
  connect = vi.fn((dest: unknown) => dest);
  disconnect = vi.fn();

  static instances: FakeWorkletNode[] = [];

  constructor() {
    FakeWorkletNode.instances.push(this);
  }
}

class FakeGainNode {
  gain: { value: number };
  connect = vi.fn((dest: unknown) => dest);
  disconnect = vi.fn();

  static instances: FakeGainNode[] = [];

  constructor(_ctx: unknown, opts: { gain: number }) {
    this.gain = { value: opts.gain };
    FakeGainNode.instances.push(this);
  }
}

async function importSink() {
  vi.resetModules();
  return await import("./sink");
}

describe("createWebAudioSink", () => {
  beforeEach(() => {
    FakeAudioContext.initialState = "running";
    FakeAudioContext.instances = [];
    FakeWorkletNode.instances = [];
    FakeGainNode.instances = [];
    createDecoder.mockReset();
    vi.stubGlobal("AudioContext", FakeAudioContext);
    vi.stubGlobal("AudioWorkletNode", FakeWorkletNode);
    vi.stubGlobal("GainNode", FakeGainNode);
    vi.stubGlobal("document", {
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      visibilityState: "visible",
    });
    // The worklet URL is never fetched in tests; the fake context's addModule ignores it.
    vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:sdr-test");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("decoder failure tears down the audio graph and ends the worklet processor", async () => {
    createDecoder.mockRejectedValue(new Error("wasm fetch failed"));
    const { createWebAudioSink } = await importSink();

    await expect(createWebAudioSink(0.5, () => {})).rejects.toThrow("wasm fetch failed");

    const node = FakeWorkletNode.instances[0];
    const gain = FakeGainNode.instances[0];
    // Without "close" the processor's process() keeps returning true forever.
    expect(node?.port.postMessage).toHaveBeenCalledWith("close");
    expect(node?.disconnect).toHaveBeenCalled();
    expect(gain?.disconnect).toHaveBeenCalled();
  });

  it("reports a suspended context, retries resume on gesture, and reports recovery", async () => {
    createDecoder.mockResolvedValue({ channels: 1, decode: vi.fn(), close: vi.fn() });
    const sinkModule = await importSink();
    const reported: boolean[] = [];
    sinkModule.onOutputStateChange((running) => reported.push(running));

    await sinkModule.createWebAudioSink(1, () => {});
    const context = FakeAudioContext.instances[0];
    expect(sinkModule.isOutputRunning()).toBe(true);

    context?.setState("suspended");
    expect(sinkModule.isOutputRunning()).toBe(false);
    expect(reported).toEqual([false]);

    // A user gesture retries the resume.
    sinkModule.resumeAudioOutput();
    expect(context?.resume).toHaveBeenCalled();
    context?.setState("running");
    expect(reported).toEqual([false, true]);
  });

  it("a context created suspended (autoplay veto) is resumed inside the creating gesture", async () => {
    createDecoder.mockResolvedValue({ channels: 1, decode: vi.fn(), close: vi.fn() });
    FakeAudioContext.initialState = "suspended";
    const sinkModule = await importSink();
    const reported: boolean[] = [];
    sinkModule.onOutputStateChange((running) => reported.push(running));

    await sinkModule.createWebAudioSink(1, () => {});
    const context = FakeAudioContext.instances[0];
    expect(context?.resume).toHaveBeenCalled();
    // Health is published immediately, not deferred to a statechange that may never fire.
    expect(reported).toEqual([false]);
    expect(sinkModule.isOutputRunning()).toBe(false);

    context?.setState("running");
    expect(reported).toEqual([false, true]);
  });

  it("conceal posts a silence buffer of exactly the gap size", async () => {
    createDecoder.mockResolvedValue({ channels: 1, decode: vi.fn(), close: vi.fn() });
    const { createWebAudioSink } = await importSink();

    const sink = await createWebAudioSink(1, () => {});
    const node = FakeWorkletNode.instances[0];
    node?.port.postMessage.mockClear();

    // The graph is two-channel, so a 480-frame gap is 960 interleaved samples of silence.
    sink.conceal(480);
    const posted = node?.port.postMessage.mock.calls[0]?.[0] as Float32Array | undefined;
    expect(posted).toBeInstanceOf(Float32Array);
    expect(posted?.length).toBe(480 * 2);
    expect(posted?.every((v) => v === 0)).toBe(true);
  });

  it("plays a mono stream on both output channels", async () => {
    let emit: ((pcm: Float32Array) => void) | undefined;
    createDecoder.mockImplementation((channels: number, onPcm: (pcm: Float32Array) => void) => {
      emit = onPcm;
      return Promise.resolve({ channels, decode: vi.fn(), close: vi.fn() });
    });
    const { createWebAudioSink } = await importSink();

    await createWebAudioSink(1, () => {});
    const node = FakeWorkletNode.instances[0];
    node?.port.postMessage.mockClear();

    emit?.(Float32Array.from([0.25, -0.5]));
    const posted = node?.port.postMessage.mock.calls[0]?.[0] as Float32Array | undefined;
    expect(Array.from(posted ?? [])).toEqual([0.25, 0.25, -0.5, -0.5]);
  });

  // A channel that switches to stereo mid-stream cannot be decoded by the mono decoder, and
  // no Opus decoder can be reconfigured in place: the sink has to build a replacement.
  it("swaps in a decoder for the packet's layout and passes its pcm through untouched", async () => {
    const decoders: {
      channels: number;
      decode: ReturnType<typeof vi.fn>;
      close: ReturnType<typeof vi.fn>;
    }[] = [];
    let emit: ((pcm: Float32Array) => void) | undefined;
    createDecoder.mockImplementation((channels: number, onPcm: (pcm: Float32Array) => void) => {
      emit = onPcm;
      const decoder = { channels, decode: vi.fn(), close: vi.fn() };
      decoders.push(decoder);
      return Promise.resolve(decoder);
    });
    const { createWebAudioSink } = await importSink();

    const sink = await createWebAudioSink(1, () => {});
    const packet = Uint8Array.from([1, 2, 3]);

    // The packet that announces the new layout is dropped, not decoded with the wrong one.
    sink.push(packet, 0, 2);
    expect(decoders[0]?.decode).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(decoders).toHaveLength(2));
    expect(decoders[1]?.channels).toBe(2);
    expect(decoders[0]?.close).toHaveBeenCalled();

    sink.push(packet, 20_000, 2);
    expect(decoders[1]?.decode).toHaveBeenCalledWith(packet, 20_000);

    const node = FakeWorkletNode.instances[0];
    node?.port.postMessage.mockClear();
    emit?.(Float32Array.from([0.25, -0.5]));
    const posted = node?.port.postMessage.mock.calls[0]?.[0] as Float32Array | undefined;
    expect(Array.from(posted ?? [])).toEqual([0.25, -0.5]);
  });
});
