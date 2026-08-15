import { describe, expect, it } from "vitest";
import { LossTracker } from "./loss";

const MAX_GAP = 19_200;

describe("LossTracker", () => {
  it("learns the packet duration from deltas and stays continuous without loss", () => {
    const t = new LossTracker(MAX_GAP);
    expect(t.next(0n)).toEqual({ kind: "continuous" });
    expect(t.next(960n)).toEqual({ kind: "continuous" });
    expect(t.next(1_920n)).toEqual({ kind: "continuous" });
    expect(t.next(2_880n)).toEqual({ kind: "continuous" });
  });

  it("reports the exact number of missing frames on a gap", () => {
    const t = new LossTracker(MAX_GAP);
    t.next(0n);
    t.next(960n);
    expect(t.next(3_840n)).toEqual({ kind: "gap", frames: 1_920 });
    expect(t.next(4_800n)).toEqual({ kind: "continuous" });
  });

  it("resets on a hole too wide to conceal, keeping the learned duration", () => {
    const t = new LossTracker(MAX_GAP);
    t.next(0n);
    t.next(960n);
    expect(t.next(960n + 960n + 20_000n)).toEqual({ kind: "reset", frames: 20_000 });
    expect(t.next(960n + 960n + 20_000n + 960n)).toEqual({ kind: "continuous" });
  });

  it("resets on a non-monotonic timestamp and relearns the duration", () => {
    const t = new LossTracker(MAX_GAP);
    t.next(0n);
    t.next(960n);
    expect(t.next(0n)).toEqual({ kind: "reset", frames: 0 });
    expect(t.next(960n)).toEqual({ kind: "continuous" });
    expect(t.next(2_880n)).toEqual({ kind: "gap", frames: 960 });
  });

  it("adopts a smaller delta when the first delta was itself a loss gap", () => {
    const t = new LossTracker(MAX_GAP);
    t.next(0n);
    expect(t.next(1_920n)).toEqual({ kind: "continuous" });
    expect(t.next(2_880n)).toEqual({ kind: "continuous" });
    expect(t.next(3_840n)).toEqual({ kind: "continuous" });
    expect(t.next(5_760n)).toEqual({ kind: "gap", frames: 960 });
  });

  it("reset() forgets history so a rebound stream starts clean", () => {
    const t = new LossTracker(MAX_GAP);
    t.next(0n);
    t.next(960n);
    t.reset();
    expect(t.next(0n)).toEqual({ kind: "continuous" });
    expect(t.next(960n)).toEqual({ kind: "continuous" });
  });
});
