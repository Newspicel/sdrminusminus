import { describe, expect, it } from "vitest";
import { JitterBuffer } from "./jitter";

function ramp(start: number, length: number): Float32Array {
  return Float32Array.from({ length }, (_, i) => start + i);
}

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

    expect(readMono(jb, 4)).toEqual({ ok: true, out: [5, 0, 0, 0] });
    expect(readMono(jb, 4)).toEqual({ ok: false, out: [0, 0, 0, 0] });
    expect(jb.targetDepth).toBe(6);

    jb.push(ramp(6, 5));
    expect(readMono(jb, 4)).toEqual({ ok: false, out: [0, 0, 0, 0] });
    jb.push(ramp(11, 1));
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
    jb.push(ramp(1, 250));
    const { maxStep } = stream(jb, 50, 1_000, 251);

    expect(jb.buffered).toBeLessThan(200);
    expect(jb.buffered).toBeGreaterThan(50);
    expect(maxStep).toBeLessThan(1.01);
  });

  it("rebuilds headroom by playing imperceptibly slow when the depth sits under target", () => {
    const jb = new JitterBuffer(100, 1000, 1);
    jb.push(ramp(1, 100));
    readMono(jb, 50);
    const { maxStep } = stream(jb, 50, 1_000, 101);

    expect(jb.buffered).toBeGreaterThan(80);
    expect(maxStep).toBeLessThan(1.01);
  });

  it("hard-sheds a backlog far too large for the drift correction to walk off", () => {
    const jb = new JitterBuffer(100, 1000, 1);
    jb.push(ramp(1, 600));
    stream(jb, 50, 60, 601);
    expect(jb.buffered).toBeLessThan(150);
  });

  it("rides out a producer clock 0.1 % fast for three minutes, discarding nothing", () => {
    const jb = new JitterBuffer(4_800, 48_000, 1);
    const { maxStep } = drift(jb, 1_000, 180);
    expect(jb.buffered).toBeLessThan(2 * 4_800);
    expect(maxStep).toBeLessThan(1.01);
    expect(jb.targetDepth).toBe(4_800);
  });

  it("rides out a producer clock 0.1 % slow for three minutes without starving", () => {
    const jb = new JitterBuffer(4_800, 48_000, 1);
    const { maxStep } = drift(jb, -1_000, 180);
    expect(jb.buffered).toBeGreaterThan(0.5 * 4_800);
    expect(maxStep).toBeLessThan(1.01);
    expect(jb.targetDepth).toBe(4_800);
  });

  it("still works when rebuilt from its own source, as the worklet rebuilds it", () => {
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
    expect(resumed.out[0]).toBe(1);
    expect(resumed.out[1]).toBeCloseTo(2, 2);
  });

  it("deinterleaves stereo frames into one output per channel", () => {
    const jb = new JitterBuffer(2, 8, 2);
    jb.push(Float32Array.from([1, -1, 2, -2, 3, -3]));
    expect(jb.buffered).toBe(3);
    expect(read(jb, 3, 2).out).toEqual([
      [1, 2, 3],
      [-1, -2, -3],
    ]);
  });

  it("counts depth, target and capacity in frames rather than samples", () => {
    const jb = new JitterBuffer(4, 8, 2);
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
