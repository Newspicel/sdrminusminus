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
    createScriptProcessor: vi.fn((frames: number) => {
      const node = new FakeScriptProcessor(frames);
      nodes.push(node);
      return node;
    }),
  };
  return { context: context as unknown as AudioContext, nodes };
}

describe("createPlayback without a secure context", () => {
  beforeEach(() => {
    vi.stubGlobal("location", { hostname: "docker01" });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads audioWorklet off a plain-HTTP context without throwing", () => {
    const { context } = insecureContext();
    expect(supportsAudioWorklet(context)).toBe(false);
  });

  it("pulls pushed samples through a script processor when no worklet is on offer", async () => {
    const { context, nodes } = insecureContext();
    const playback = await createPlayback(context, () => {});
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
    const playback = await createPlayback(context, (report) => reports.push(report));
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
    const playback = await createPlayback(context, () => {});
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
    await createPlayback(context, (report) => reports.push(report));
    const node = nodes[0];
    const frames = node?.frames ?? 1;
    for (let pull = 0; pull * frames < SAMPLE_RATE / 2; pull++) {
      node?.pull();
    }
    expect(reports[0]?.targetFrames).toBe(0.1 * SAMPLE_RATE);
  });
});
