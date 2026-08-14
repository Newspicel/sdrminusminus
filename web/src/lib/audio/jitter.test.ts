import { describe, expect, it } from "vitest";
import { JitterBuffer } from "./jitter";

function ramp(start: number, length: number): Float32Array {
  return Float32Array.from({ length }, (_, i) => start + i);
}

/** Read `frames` into one output buffer per channel. */
function read(
  jb: JitterBuffer,
  frames: number,
  channels: number,
): { ok: boolean; out: number[][] } {
  const outputs = Array.from({ length: channels }, () => new Float32Array(frames));
  const ok = jb.read(outputs);
  return { ok, out: outputs.map((o) => Array.from(o)) };
}

function readMono(jb: JitterBuffer, frames: number): { ok: boolean; out: number[] } {
  const { ok, out } = read(jb, frames, 1);
  return { ok, out: out[0] ?? [] };
}

/** Steady state: `frames` in and `frames` out per iteration, tracking the largest jump. */
function stream(
  jb: JitterBuffer,
  frames: number,
  iterations: number,
  from: number,
): { maxStep: number } {
  let next = from;
  let last = Number.NaN;
  let maxStep = 0;
  for (let i = 0; i < iterations; i++) {
    for (const v of readMono(jb, frames).out) {
      if (!Number.isNaN(last)) {
        maxStep = Math.max(maxStep, v - last);
      }
      last = v;
    }
    jb.push(ramp(next, frames));
    next += frames;
  }
  return { maxStep };
}

/**
 * The real cadence: 20 ms Opus packets in, 128-frame render quanta out, at 48 kHz — with the
 * producer's clock off by `ppm`, which is what a radio's sample clock and a sound card's
 * always are relative to each other. Values wrap well below float32's integer limit so a
 * discarded chunk still shows as a step.
 */
function drift(jb: JitterBuffer, ppm: number, seconds: number): { maxStep: number } {
  const quantum = 128;
  const packet = 960;
  const out = new Float32Array(quantum);
  let owed = 0;
  let next = 0;
  let last = Number.NaN;
  let maxStep = 0;
  for (let q = 0; q < Math.floor((seconds * 48_000) / quantum); q++) {
    owed += quantum * (1 + ppm / 1e6);
    while (owed >= packet) {
      jb.push(Float32Array.from({ length: packet }, (_, i) => (next + i) % 100_000));
      next += packet;
      owed -= packet;
    }
    if (jb.read([out])) {
      for (const v of out) {
        if (!Number.isNaN(last)) {
          maxStep = Math.max(maxStep, v - last);
        }
        last = v;
      }
    }
  }
  return { maxStep };
}

describe("JitterBuffer", () => {
  it("outputs silence until the target fill is reached", () => {
    const jb = new JitterBuffer(4, 16, 1);
    jb.push(ramp(1, 3));
    expect(readMono(jb, 4)).toEqual({ ok: false, out: [0, 0, 0, 0] });
    expect(jb.buffered).toBe(3);

    jb.push(ramp(4, 2));
    expect(readMono(jb, 4)).toEqual({ ok: true, out: [1, 2, 3, 4] });
  });

  it("underruns to silence, grows the target, and rebuffers to the grown one", () => {
    const jb = new JitterBuffer(4, 16, 1);
    jb.push(ramp(1, 5));
    expect(readMono(jb, 4).out).toEqual([1, 2, 3, 4]);

    // Partial fill: real samples then silence, then buffering until target again.
    expect(readMono(jb, 4)).toEqual({ ok: true, out: [5, 0, 0, 0] });
    expect(readMono(jb, 4)).toEqual({ ok: false, out: [0, 0, 0, 0] });
    // The pre-buffer was demonstrably too small for this stream: hold more next time.
    expect(jb.targetDepth).toBe(6);

    jb.push(ramp(6, 5));
    expect(readMono(jb, 4)).toEqual({ ok: false, out: [0, 0, 0, 0] });
    jb.push(ramp(11, 1));
    // Resumed a hair slow to rebuild the headroom the underrun cost: contiguous audio from
    // where playback stopped, resampled rather than spliced.
    const resumed = readMono(jb, 4).out;
    expect(resumed[0]).toBe(6);
    expect(resumed[3]).toBeCloseTo(9, 1);
  });

  it("stops growing the target at its ceiling", () => {
    const jb = new JitterBuffer(4, 16, 1);
    for (let i = 0; i < 10; i++) {
      jb.push(ramp(1, jb.targetDepth));
      readMono(jb, jb.targetDepth + 1);
    }
    expect(jb.targetDepth).toBe(8);
  });

  it("relaxes the grown target back down after a long clean run", () => {
    const jb = new JitterBuffer(4, 16, 1);
    jb.push(ramp(1, 4));
    readMono(jb, 5);
    expect(jb.targetDepth).toBe(6);

    // relaxAfter is 1200 frames of underrun-free playback for a 4-frame floor.
    let next = 1;
    for (let i = 0; i < 320; i++) {
      jb.push(ramp(next, 4));
      next += 4;
      readMono(jb, 4);
    }
    expect(jb.targetDepth).toBe(4);
  });

  it("re-enters buffering after an exact drain", () => {
    const jb = new JitterBuffer(4, 16, 1);
    jb.push(ramp(1, 4));
    expect(readMono(jb, 4).ok).toBe(true);
    expect(jb.buffered).toBe(0);
    expect(readMono(jb, 4)).toEqual({ ok: false, out: [0, 0, 0, 0] });

    jb.push(ramp(5, 3));
    expect(readMono(jb, 4).ok).toBe(false);
  });

  it("a push past capacity sheds the oldest samples back toward target, not merely under the cap", () => {
    const jb = new JitterBuffer(2, 8, 1);
    jb.push(ramp(1, 6));
    jb.push(ramp(7, 6));
    // All 6 old samples dropped (target is 2, the new chunk alone exceeds it): depth resumes
    // at the chunk size, not parked at the 8-sample cap.
    expect(jb.buffered).toBe(6);
    expect(readMono(jb, 6).out).toEqual([7, 8, 9, 10, 11, 12]);
  });

  it("a burst past capacity resumes at target depth", () => {
    const jb = new JitterBuffer(4, 16, 1);
    jb.push(ramp(1, 14));
    jb.push(ramp(15, 4));
    expect(jb.buffered).toBe(4);
    expect(readMono(jb, 4).out).toEqual([15, 16, 17, 18]);
  });

  it("sheds sustained backlog by playing imperceptibly fast, never by discarding audio", () => {
    const jb = new JitterBuffer(100, 1000, 1);
    // A burst parked the depth at 2.5x target: inflow equals outflow from here, so only the
    // drift correction can bring it back down.
    jb.push(ramp(1, 250));
    const { maxStep } = stream(jb, 50, 1_000, 251);

    expect(jb.buffered).toBeLessThan(200);
    expect(jb.buffered).toBeGreaterThan(50);
    // The ramp is contiguous throughout: a discarded chunk would show as a jump, and the
    // resampling itself never advances by more than the 1.004 rate cap.
    expect(maxStep).toBeLessThan(1.01);
  });

  it("rebuilds headroom by playing imperceptibly slow when the depth sits under target", () => {
    const jb = new JitterBuffer(100, 1000, 1);
    jb.push(ramp(1, 100));
    // Half the target of depth, held there by equal inflow and outflow until the correction
    // walks it back up.
    readMono(jb, 50);
    const { maxStep } = stream(jb, 50, 1_000, 101);

    expect(jb.buffered).toBeGreaterThan(80);
    expect(maxStep).toBeLessThan(1.01);
  });

  it("hard-sheds a backlog far too large for the drift correction to walk off", () => {
    const jb = new JitterBuffer(100, 1000, 1);
    jb.push(ramp(1, 600));
    stream(jb, 50, 60, 601);
    // 6x target is latency, not jitter headroom, and 0.4 % would need minutes to shed it.
    expect(jb.buffered).toBeLessThan(150);
  });

  it("rides out a producer clock 0.1 % fast for three minutes, discarding nothing", () => {
    const jb = new JitterBuffer(4_800, 48_000, 1);
    const { maxStep } = drift(jb, 1_000, 180);
    // A fixed-rate reader would have accumulated ~8600 frames of backlog by now and shed it in
    // one audible splice; the correction parks the depth just above target instead.
    expect(jb.buffered).toBeLessThan(2 * 4_800);
    expect(maxStep).toBeLessThan(1.01);
    // The target only grows on an underrun, so an untouched floor means playback never starved.
    expect(jb.targetDepth).toBe(4_800);
  });

  it("rides out a producer clock 0.1 % slow for three minutes without starving", () => {
    const jb = new JitterBuffer(4_800, 48_000, 1);
    const { maxStep } = drift(jb, -1_000, 180);
    // The same backlog in the other direction: a fixed-rate reader would have run dry twice.
    expect(jb.buffered).toBeGreaterThan(0.5 * 4_800);
    expect(maxStep).toBeLessThan(1.01);
    expect(jb.targetDepth).toBe(4_800);
  });

  it("still works when rebuilt from its own source, as the worklet rebuilds it", () => {
    // worklet.ts ships this class to the audio thread as `JitterBuffer.toString()`, where
    // module scope does not exist: anything it referenced from outside its own body — an
    // import, a module constant, a static of its own — would be a ReferenceError there.
    // oxlint-disable-next-line typescript/no-implied-eval -- evaluating that source is precisely what is under test.
    const rebuild = new Function(`"use strict"; return (${JitterBuffer.toString()});`);
    const Rebuilt = rebuild() as typeof JitterBuffer;

    const jb = new Rebuilt(4, 16, 1);
    jb.push(ramp(1, 4));
    expect(readMono(jb, 4)).toEqual({ ok: true, out: [1, 2, 3, 4] });
    expect(jb.targetDepth).toBe(4);
  });

  it("keeps only the newest samples of a chunk larger than capacity", () => {
    const jb = new JitterBuffer(2, 4, 1);
    jb.push(ramp(1, 10));
    expect(jb.buffered).toBe(4);
    expect(readMono(jb, 4).out).toEqual([7, 8, 9, 10]);
  });

  it("stays correct across ring wraparound", () => {
    const jb = new JitterBuffer(2, 5, 1);
    jb.push(ramp(1, 4));
    expect(readMono(jb, 3).out).toEqual([1, 2, 3]);
    jb.push(ramp(5, 4));
    expect(readMono(jb, 5).out).toEqual([4, 5, 6, 7, 8]);
  });

  it("clear resets to an empty buffering state", () => {
    const jb = new JitterBuffer(2, 8, 1);
    jb.push(ramp(1, 4));
    jb.clear();
    expect(jb.buffered).toBe(0);
    expect(readMono(jb, 4)).toEqual({ ok: false, out: [0, 0, 0, 0] });
    jb.push(ramp(1, 2));
    const resumed = readMono(jb, 2);
    expect(resumed.ok).toBe(true);
    // Rebuffering leaves the depth average below target, so playback resumes a hair slow:
    // sample-exact on the first frame, fractionally behind on the next.
    expect(resumed.out[0]).toBe(1);
    expect(resumed.out[1]).toBeCloseTo(2, 2);
  });

  it("deinterleaves stereo frames into one output per channel", () => {
    const jb = new JitterBuffer(2, 8, 2);
    // Left counts up, right counts down: a swapped or shifted lane is visible in the values.
    jb.push(Float32Array.from([1, -1, 2, -2, 3, -3]));
    expect(jb.buffered).toBe(3);
    expect(read(jb, 3, 2).out).toEqual([
      [1, 2, 3],
      [-1, -2, -3],
    ]);
  });

  it("counts depth, target and capacity in frames rather than samples", () => {
    const jb = new JitterBuffer(4, 8, 2);
    // Three stereo frames is six samples, and must still be under a four-frame target.
    jb.push(Float32Array.from([1, -1, 2, -2, 3, -3]));
    expect(jb.buffered).toBe(3);
    expect(read(jb, 4, 2).ok).toBe(false);

    jb.push(Float32Array.from([4, -4]));
    expect(read(jb, 4, 2)).toEqual({
      ok: true,
      out: [
        [1, 2, 3, 4],
        [-1, -2, -3, -4],
      ],
    });
  });

  it("stays correct across ring wraparound in stereo", () => {
    const jb = new JitterBuffer(2, 3, 2);
    jb.push(Float32Array.from([1, -1, 2, -2]));
    expect(read(jb, 1, 2).out).toEqual([[1], [-1]]);
    jb.push(Float32Array.from([3, -3, 4, -4]));
    expect(read(jb, 3, 2).out).toEqual([
      [2, 3, 4],
      [-2, -3, -4],
    ]);
  });

  it("feeds every output channel from a stream with fewer of them", () => {
    const jb = new JitterBuffer(2, 8, 1);
    jb.push(ramp(1, 3));
    expect(read(jb, 3, 2).out).toEqual([
      [1, 2, 3],
      [1, 2, 3],
    ]);
  });
});
