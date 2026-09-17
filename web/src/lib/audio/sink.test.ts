import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { createDecoder } = vi.hoisted(() => ({ createDecoder: vi.fn() }));
vi.mock("./decoder", () => ({ createOpusPacketDecoder: createDecoder }));

class FakeAudioContext {
  static initialState = "running";
  static worklet = true;
  static instances: FakeAudioContext[] = [];

  state = FakeAudioContext.initialState;
  currentTime = 0;
  destination = {};
  audioWorklet = FakeAudioContext.worklet
    ? { addModule: vi.fn(() => Promise.resolve()) }
    : undefined;
  resume = vi.fn(() => Promise.resolve());
  createScriptProcessor = vi.fn((frames: number) => new FakeScriptProcessor(frames));
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

class FakeScriptProcessor {
  onaudioprocess: ((event: { outputBuffer: unknown }) => void) | null = null;
  connect = vi.fn((dest: unknown) => dest);
  disconnect = vi.fn();

  static instances: FakeScriptProcessor[] = [];

  constructor(readonly frames: number) {
    FakeScriptProcessor.instances.push(this);
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
  gain: { value: number; setTargetAtTime: (value: number) => void };
  connect = vi.fn((dest: unknown) => dest);
  disconnect = vi.fn();

  static instances: FakeGainNode[] = [];

  constructor(_ctx: unknown, opts: { gain: number }) {
    this.gain = {
      value: opts.gain,
      setTargetAtTime: (value: number) => {
        this.gain.value = value;
      },
    };
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
    FakeAudioContext.worklet = true;
    FakeAudioContext.instances = [];
    FakeScriptProcessor.instances = [];
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
    vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:sdr-test");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("decoder failure tears down the audio graph and ends the worklet processor", async () => {
    createDecoder.mockRejectedValue(new Error("wasm fetch failed"));
    const { createWebAudioSink } = await importSink();

    await expect(
      createWebAudioSink(
        "1:1",
        0.5,
        () => {},
        () => {},
      ),
    ).rejects.toThrow("wasm fetch failed");

    const node = FakeWorkletNode.instances[0];
    const gain = FakeGainNode.instances[0];
    expect(node?.port.postMessage).toHaveBeenCalledWith("close");
    expect(node?.disconnect).toHaveBeenCalled();
    expect(gain?.disconnect).toHaveBeenCalled();
  });

  it("reports a suspended context, retries resume on gesture, and reports recovery", async () => {
    createDecoder.mockResolvedValue({
      channels: 1,
      decode: vi.fn(),
      reset: vi.fn(),
      close: vi.fn(),
    });
    const sinkModule = await importSink();
    const reported: boolean[] = [];
    sinkModule.onOutputStateChange((running) => reported.push(running));

    await sinkModule.createWebAudioSink(
      "1:1",
      1,
      () => {},
      () => {},
    );
    const context = FakeAudioContext.instances[0];
    expect(sinkModule.isOutputRunning()).toBe(true);

    context?.setState("suspended");
    expect(sinkModule.isOutputRunning()).toBe(false);
    expect(reported).toEqual([false]);

    sinkModule.resumeAudioOutput();
    expect(context?.resume).toHaveBeenCalled();
    context?.setState("running");
    expect(reported).toEqual([false, true]);
  });

  it("a context created suspended (autoplay veto) is resumed inside the creating gesture", async () => {
    createDecoder.mockResolvedValue({
      channels: 1,
      decode: vi.fn(),
      reset: vi.fn(),
      close: vi.fn(),
    });
    FakeAudioContext.initialState = "suspended";
    const sinkModule = await importSink();
    const reported: boolean[] = [];
    sinkModule.onOutputStateChange((running) => reported.push(running));

    await sinkModule.createWebAudioSink(
      "1:1",
      1,
      () => {},
      () => {},
    );
    const context = FakeAudioContext.instances[0];
    expect(context?.resume).toHaveBeenCalled();
    expect(reported).toEqual([false]);
    expect(sinkModule.isOutputRunning()).toBe(false);

    context?.setState("running");
    expect(reported).toEqual([false, true]);
  });

  it("conceal posts a silence buffer of exactly the gap size", async () => {
    createDecoder.mockResolvedValue({
      channels: 1,
      decode: vi.fn(),
      reset: vi.fn(),
      close: vi.fn(),
    });
    const { createWebAudioSink } = await importSink();

    const sink = await createWebAudioSink(
      "1:1",
      1,
      () => {},
      () => {},
    );
    const node = FakeWorkletNode.instances[0];
    node?.port.postMessage.mockClear();

    sink.conceal(480);
    const posted = node?.port.postMessage.mock.calls[0]?.[0] as Float32Array | undefined;
    expect(posted).toBeInstanceOf(Float32Array);
    expect(posted?.length).toBe(480 * 2);
    expect(posted?.every((v) => v === 0)).toBe(true);
  });

  it("requests stereo decoder output and sends it without another allocation", async () => {
    let emit: ((pcm: Float32Array) => void) | undefined;
    createDecoder.mockImplementation((channels: number, onPcm: (pcm: Float32Array) => void) => {
      emit = onPcm;
      return Promise.resolve({ channels, decode: vi.fn(), close: vi.fn() });
    });
    const { createWebAudioSink } = await importSink();

    await createWebAudioSink(
      "1:1",
      1,
      () => {},
      () => {},
    );
    const node = FakeWorkletNode.instances[0];
    node?.port.postMessage.mockClear();

    expect(createDecoder.mock.calls[0]?.[0]).toBe(2);
    const pcm = Float32Array.from([0.25, 0.25, -0.5, -0.5]);
    emit?.(pcm);
    const posted = node?.port.postMessage.mock.calls[0]?.[0] as Float32Array | undefined;
    expect(posted).toBe(pcm);
    expect(Array.from(posted ?? [])).toEqual([0.25, 0.25, -0.5, -0.5]);
  });

  it("accepts the first stereo packet and repeated layout changes without replacing the decoder", async () => {
    const decoders: {
      channels: number;
      decode: ReturnType<typeof vi.fn>;
      close: ReturnType<typeof vi.fn>;
    }[] = [];
    let emit: ((pcm: Float32Array) => void) | undefined;
    createDecoder.mockImplementation((channels: number, onPcm: (pcm: Float32Array) => void) => {
      emit = onPcm;
      const decoder = { channels, decode: vi.fn(() => true), close: vi.fn() };
      decoders.push(decoder);
      return Promise.resolve(decoder);
    });
    const { createWebAudioSink } = await importSink();

    const sink = await createWebAudioSink(
      "1:1",
      1,
      () => {},
      () => {},
    );
    const packet = Uint8Array.from([1, 2, 3]);

    for (let index = 0; index < 1000; index++) {
      expect(sink.push(packet, index * 20_000, index % 2 === 0 ? 2 : 1)).toBe(true);
    }
    expect(decoders).toHaveLength(1);
    expect(decoders[0]?.channels).toBe(2);
    expect(decoders[0]?.close).not.toHaveBeenCalled();
    expect(decoders[0]?.decode).toHaveBeenCalledTimes(1000);
    expect(decoders[0]?.decode).toHaveBeenNthCalledWith(1, packet, 0);
    expect(decoders[0]?.decode).toHaveBeenLastCalledWith(packet, 19_980_000);

    const node = FakeWorkletNode.instances[0];
    node?.port.postMessage.mockClear();
    emit?.(Float32Array.from([0.25, -0.5]));
    const posted = node?.port.postMessage.mock.calls[0]?.[0] as Float32Array | undefined;
    expect(Array.from(posted ?? [])).toEqual([0.25, -0.5]);
  });

  it("reports unsupported layouts without starting another decoder", async () => {
    const decode = vi.fn(() => true);
    createDecoder.mockResolvedValue({ channels: 2, decode, close: vi.fn() });
    const { createWebAudioSink } = await importSink();
    const error = vi.fn();
    const sink = await createWebAudioSink("1:1", 1, error, vi.fn());
    expect(sink.push(new Uint8Array([1]), 0, 3)).toBe(false);
    expect(error).toHaveBeenCalledExactlyOnceWith(new Error("Unsupported audio channel count: 3"));
    expect(decode).not.toHaveBeenCalled();
    expect(createDecoder).toHaveBeenCalledTimes(1);
    sink.close();
  });

  it("plays through a script processor on an origin that withholds AudioWorklet", async () => {
    FakeAudioContext.worklet = false;
    vi.stubGlobal("AudioWorkletNode", undefined);
    let emit: ((pcm: Float32Array) => void) | undefined;
    createDecoder.mockImplementation((channels: number, onPcm: (pcm: Float32Array) => void) => {
      emit = onPcm;
      return Promise.resolve({ channels, decode: vi.fn(), reset: vi.fn(), close: vi.fn() });
    });
    const { createWebAudioSink } = await importSink();

    const sink = await createWebAudioSink(
      "1:1",
      0.5,
      () => {},
      () => {},
    );

    const node = FakeScriptProcessor.instances[0];
    const gain = FakeGainNode.instances[0];
    expect(node).toBeDefined();
    expect(FakeWorkletNode.instances).toHaveLength(0);
    expect(node?.connect).toHaveBeenCalledWith(gain);
    expect(() => emit?.(Float32Array.from([0.25, -0.5]))).not.toThrow();

    sink.close();
    expect(node?.onaudioprocess).toBeNull();
    expect(node?.disconnect).toHaveBeenCalled();
    expect(gain?.disconnect).toHaveBeenCalled();
  });

  it("tapers the slider position onto a 60 dB gain curve at creation and on change", async () => {
    createDecoder.mockResolvedValue({
      channels: 1,
      decode: vi.fn(),
      reset: vi.fn(),
      close: vi.fn(),
    });
    const { createWebAudioSink, gainForVolume } = await importSink();

    expect(gainForVolume(0)).toBe(0);
    expect(gainForVolume(1)).toBeCloseTo(1, 6);
    expect(gainForVolume(0.5)).toBeCloseTo(10 ** -1.5, 6);
    expect(gainForVolume(-1)).toBe(0);
    expect(gainForVolume(2)).toBeCloseTo(1, 6);
    for (const step of [0.1, 0.4, 0.8]) {
      expect(gainForVolume(step)).toBeLessThan(gainForVolume(step + 0.1));
    }

    const sink = await createWebAudioSink(
      "1:1",
      0.5,
      () => {},
      () => {},
    );
    const gain = FakeGainNode.instances[0];
    expect(gain?.gain.value).toBeCloseTo(gainForVolume(0.5), 6);

    sink.setVolume(0.25);
    expect(gain?.gain.value).toBeCloseTo(gainForVolume(0.25), 6);
  });
});
