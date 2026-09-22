import { describe, expect, it } from "vitest";
import { FrameTween } from "./frameTween";

describe("FrameTween", () => {
  it("shows the first frame as it came", () => {
    const tween = new FrameTween();
    tween.push(Float32Array.from([-80, -40]), 0);
    expect([...tween.sample(0)]).toEqual([-80, -40]);
  });

  it("blends toward a new frame over one frame interval", () => {
    const tween = new FrameTween();
    tween.push(Float32Array.of(-80), 0);
    tween.push(Float32Array.of(-60), 1000 / 30);
    const start = 1000 / 30;
    expect(tween.sample(start)[0]).toBeCloseTo(-80);
    expect(tween.sample(start + 1000 / 60)[0]).toBeCloseTo(-70, 0);
    expect(tween.sample(start + 1000)[0]).toBeCloseTo(-60);
  });

  it("starts the next blend where the shown trace is", () => {
    const tween = new FrameTween();
    tween.push(Float32Array.of(-80), 0);
    tween.push(Float32Array.of(-60), 1000 / 30);
    const mid = 1000 / 30 + 1000 / 60;
    const shown = tween.sample(mid)[0] ?? 0;
    tween.push(Float32Array.of(-20), mid);
    expect(tween.sample(mid)[0]).toBeCloseTo(shown);
  });

  it("jumps without blending after a retune", () => {
    const tween = new FrameTween();
    tween.push(Float32Array.of(-80), 0);
    tween.jump(Float32Array.of(-10), 5);
    expect(tween.sample(5)[0]).toBe(-10);
  });

  it("restarts when the bin count changes", () => {
    const tween = new FrameTween();
    tween.push(Float32Array.of(-80), 0);
    tween.push(Float32Array.of(-50, -40), 10);
    expect([...tween.sample(10)]).toEqual([-50, -40]);
  });
});
