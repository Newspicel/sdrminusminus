import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPlayback, supportsAudioWorklet } from "./playback";
import { CHANNELS, MAX_FRAMES, SAMPLE_RATE, type WorkletReport } from "./worklet";

class FakeOutputBuffer {
  readonly lanes: Float32Array[];

  constructor(
    readonly numberOfChannels: number,
    readonly length: number,
  ) {
    this.lanes = Array.from({ length: numberOfChannels }, () => new Float32Array(length));
  }

  getChannelData(channel: number): Float32Array {
    const lane = this.lanes[channel];
    if (!lane) {
      throw new Error(`no lane ${channel}`);
    }
    return lane;
  }
}

class FakeScriptProcessor {
  onaudioprocess: ((event: { outputBuffer: FakeOutputBuffer }) => void) | null = null;
  disconnect = vi.fn();
  connect = vi.fn((dest: unknown) => dest);

  constructor(readonly frames: number) {}

  pull(): FakeOutputBuffer {
    const outputBuffer = new FakeOutputBuffer(CHANNELS, this.frames);
    this.onaudioprocess?.({ outputBuffer });
    return outputBuffer;
  }
}

function insecureContext(): { context: AudioContext; nodes: FakeScriptProcessor[] } {
  const nodes: FakeScriptProcessor[] = [];
  const context = {
    get currentTime() {
      return performance.now() / 1000;
    },
    getOutputTimestamp: () => ({ contextTime: performance.now() / 1000 }),
    createScriptProcessor: vi.fn((frames: number) => {
      const node = new FakeScriptProcessor(frames);
      nodes.push(node);
      return node;
    }),
  };
  return { context: context as unknown as AudioContext, nodes };
}

class FakeWorkletNode {
  static instances: FakeWorkletNode[] = [];
  port = {
    onmessage: null as ((event: MessageEvent<WorkletReport>) => void) | null,
    postMessage: vi.fn(),
  };
  onprocessorerror: (() => void) | null = null;
  disconnect = vi.fn();

  constructor() {
    FakeWorkletNode.instances.push(this);
  }
}

function workletContext() {
  return {
    currentTime: 0,
    audioWorklet: { addModule: vi.fn(() => Promise.resolve()) },
  } as unknown as AudioContext & { audioWorklet: { addModule: ReturnType<typeof vi.fn> } };
}

describe("createPlayback with an audio worklet", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    FakeWorkletNode.instances = [];
    vi.stubGlobal("AudioWorkletNode", FakeWorkletNode);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("waits for the device clock after an initial render callback and cancels polling on release", async () => {
    vi.useFakeTimers();
    let outputTime = 0;
    const timestamp = vi.fn(() => ({ contextTime: outputTime }));
    const clock = {
      currentTime: 0.005,
      getOutputTimestamp: timestamp,
      audioWorklet: { addModule: vi.fn(() => Promise.resolve()) },
    };
    const context = clock as unknown as AudioContext;
    const playback = await createPlayback(context, vi.fn(), vi.fn());
    const node = FakeWorkletNode.instances[0];
    if (!node) throw new Error("worklet missing");
    const ready = vi.fn();
    void playback.ready.then(ready);
    node.port.onmessage?.({ data: { underruns: 0 } } as MessageEvent<WorkletReport>);
    await vi.advanceTimersByTimeAsync(750);
    expect(ready).not.toHaveBeenCalled();
    outputTime = 0.01;
    clock.currentTime = 0.01;
    await vi.advanceTimersByTimeAsync(750);
    expect(ready).not.toHaveBeenCalled();
    outputTime = 0.2;
    await vi.advanceTimersByTimeAsync(10);
    expect(ready).not.toHaveBeenCalled();
    clock.currentTime = 0.2;
    await vi.advanceTimersByTimeAsync(10);
    expect(ready).toHaveBeenCalledTimes(1);
    playback.release();

    outputTime = 0;
    const cancelled = await createPlayback(context, vi.fn(), vi.fn());
    FakeWorkletNode.instances[1]?.port.onmessage?.({
      data: { underruns: 0 },
    } as MessageEvent<WorkletReport>);
    cancelled.release();
    const calls = timestamp.mock.calls.length;
    await vi.advanceTimersByTimeAsync(1000);
    expect(timestamp).toHaveBeenCalledTimes(calls);
  });

  it("registers the processor once per context", async () => {
    const first = workletContext();
    const second = workletContext();
    (await createPlayback(first, vi.fn(), vi.fn())).release();
    (await createPlayback(first, vi.fn(), vi.fn())).release();
    (await createPlayback(second, vi.fn(), vi.fn())).release();
    expect(first.audioWorklet.addModule).toHaveBeenCalledOnce();
    expect(second.audioWorklet.addModule).toHaveBeenCalledOnce();
  });

  it.each([false, true])("reports processor failure with ready=%s", async (started) => {
    const context = {
      get currentTime() {
        return performance.now() / 1000;
      },
      getOutputTimestamp: () => ({ contextTime: performance.now() / 1000 }),
      audioWorklet: { addModule: vi.fn(() => Promise.resolve()) },
    } as unknown as AudioContext;
    const error = vi.fn();
    const playback = await createPlayback(context, vi.fn(), error);
    const node = FakeWorkletNode.instances[0];
    if (!node) throw new Error("worklet missing");
    if (started) {
      node.port.onmessage?.({ data: { underruns: 0 } } as MessageEvent<WorkletReport>);
      await vi.advanceTimersByTimeAsync(150);
      await playback.ready;
    }
    node.onprocessorerror?.();
    expect(error).toHaveBeenCalledExactlyOnceWith(new Error("Audio playback processor failed"));
    playback.release();
    expect(node.onprocessorerror).toBeNull();
    expect(node.port.onmessage).toBeNull();
  });
});

describe("createPlayback without a secure context", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("location", { hostname: "docker01" });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it("reads audioWorklet off a plain-HTTP context without throwing", () => {
    const { context } = insecureContext();
    expect(supportsAudioWorklet(context)).toBe(false);
  });

  it.each([false, true])(
    "waits for script processor clock progress with timestamps=%s",
    async (timestamps) => {
      const { context, nodes } = insecureContext();
      if (!timestamps) Object.defineProperty(context, "getOutputTimestamp", { value: undefined });
      const playback = await createPlayback(context, vi.fn(), vi.fn());
      const ready = vi.fn();
      void playback.ready.then(ready);
      await Promise.resolve();
      expect(ready).not.toHaveBeenCalled();
      nodes[0]?.pull();
      await vi.advanceTimersByTimeAsync(150);
      await playback.ready;
      expect(ready).toHaveBeenCalledTimes(1);
      playback.release();
    },
  );

  it("pulls pushed samples through a script processor when no worklet is on offer", async () => {
    const { context, nodes } = insecureContext();
    const playback = await createPlayback(context, () => {}, vi.fn());
    const node = nodes[0];
    expect(node).toBeDefined();
    expect(playback.node).toBe(node);

    const frames = node?.frames ?? 0;
    const pcm = new Float32Array(frames * CHANNELS);
    for (let frame = 0; frame < frames; frame++) {
      pcm[frame * CHANNELS] = 0.5;
      pcm[frame * CHANNELS + 1] = -0.5;
    }
    for (let block = 0; block * frames < MAX_FRAMES / 2; block++) {
      playback.send(pcm.slice());
    }

    let heard = new FakeOutputBuffer(CHANNELS, 0);
    for (let attempt = 0; attempt < 4 && heard.lanes[0]?.[0] !== 0.5; attempt++) {
      heard = node?.pull() ?? heard;
    }
    expect(heard.lanes[0]?.[0]).toBeCloseTo(0.5, 5);
    expect(heard.lanes[1]?.[0]).toBeCloseTo(-0.5, 5);
  });

  it("reports underruns and depth the same way the worklet does", async () => {
    const { context, nodes } = insecureContext();
    const reports: WorkletReport[] = [];
    const playback = await createPlayback(context, (report) => reports.push(report), vi.fn());
    const node = nodes[0];

    const frames = node?.frames ?? 1;
    for (let pull = 0; pull * frames < SAMPLE_RATE / 2; pull++) {
      node?.pull();
    }
    expect(reports).toHaveLength(1);
    expect(reports[0]?.underruns).toBe(0);
    expect(reports[0]?.bufferedFrames).toBe(0);
    expect(reports[0]?.targetFrames).toBeGreaterThan(0);

    for (let block = 0; block * frames < 2 * (reports[0]?.targetFrames ?? frames); block++) {
      playback.send(new Float32Array(frames * CHANNELS).fill(0.25));
    }
    expect(reports.at(-1)?.underruns).toBe(0);
    for (let pull = 0; pull < 8; pull++) {
      node?.pull();
    }
    expect(reports.at(-1)?.underruns).toBe(1);
  });

  it("stops pulling after close and starts over after reset", async () => {
    const { context, nodes } = insecureContext();
    const playback = await createPlayback(context, () => {}, vi.fn());
    const node = nodes[0];
    const frames = node?.frames ?? 0;

    playback.send(new Float32Array(frames * CHANNELS).fill(0.25));
    playback.send("reset");
    playback.send(new Float32Array(frames * CHANNELS).fill(0.25));
    playback.send("close");

    const silent = node?.pull();
    expect(silent?.lanes[0]?.every((sample) => sample === 0)).toBe(true);

    playback.release();
    expect(node?.onaudioprocess).toBeNull();
    expect(node?.disconnect).toHaveBeenCalled();
  });

  it("holds the remote latency budget the worklet would have used", async () => {
    const { context, nodes } = insecureContext();
    const reports: WorkletReport[] = [];
    await createPlayback(context, (report) => reports.push(report), vi.fn());
    const node = nodes[0];
    const frames = node?.frames ?? 1;
    for (let pull = 0; pull * frames < SAMPLE_RATE / 2; pull++) {
      node?.pull();
    }
    expect(reports[0]?.targetFrames).toBe(0.1 * SAMPLE_RATE);
  });
});
