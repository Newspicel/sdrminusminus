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

describe("JitterBuffer", () => {
  it("outputs silence until the target fill is reached", () => {
    const jb = new JitterBuffer(4, 16, 1);
    jb.push(ramp(1, 3));
    expect(readMono(jb, 4)).toEqual({ ok: false, out: [0, 0, 0, 0] });
    expect(jb.buffered).toBe(3);

    jb.push(ramp(4, 2));
    expect(readMono(jb, 4)).toEqual({ ok: true, out: [1, 2, 3, 4] });
  });

  it("underruns to silence and rebuffers to target before resuming", () => {
    const jb = new JitterBuffer(4, 16, 1);
    jb.push(ramp(1, 5));
    expect(readMono(jb, 4).out).toEqual([1, 2, 3, 4]);

    // Partial fill: real samples then silence, then buffering until target again.
    expect(readMono(jb, 4)).toEqual({ ok: true, out: [5, 0, 0, 0] });
    expect(readMono(jb, 4)).toEqual({ ok: false, out: [0, 0, 0, 0] });

    jb.push(ramp(6, 3));
    expect(readMono(jb, 4)).toEqual({ ok: false, out: [0, 0, 0, 0] });
    jb.push(ramp(9, 1));
    expect(readMono(jb, 4)).toEqual({ ok: true, out: [6, 7, 8, 9] });
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

  it("sheds sustained sub-cap backlog back to the target", () => {
    // trimAbove = 8, trimHold = 40 pushed samples for (target 4, max 16).
    const jb = new JitterBuffer(4, 16, 1);
    jb.push(ramp(1, 10));
    let next = 11;
    // Steady state: inflow equals outflow, but depth is parked at 10 (2.5x target).
    for (let i = 0; i < 7; i++) {
      readMono(jb, 4);
      jb.push(ramp(next, 4));
      next += 4;
      expect(jb.buffered).toBe(10);
    }
    // The sustained-high streak crosses trimHold on this push: shed to target.
    readMono(jb, 4);
    jb.push(ramp(next, 4));
    expect(jb.buffered).toBe(4);
    expect(readMono(jb, 4).out).toEqual([39, 40, 41, 42]);
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
    expect(readMono(jb, 2)).toEqual({ ok: true, out: [1, 2] });
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
