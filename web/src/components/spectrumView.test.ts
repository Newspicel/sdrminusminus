import { describe, expect, it } from "vitest";
import {
  decibelTicks,
  FULL_VIEW,
  frequencyTicks,
  isFullView,
  niceStep,
  offsetToSpan,
  panView,
  spanToView,
  viewToSpan,
  viewWidth,
  zoomView,
} from "./spectrumView";

describe("zoomView", () => {
  it("holds the frequency under the cursor still", () => {
    const before = viewToSpan(FULL_VIEW, 0.25);
    const zoomed = zoomView(FULL_VIEW, 0.25, 4);
    expect(viewToSpan(zoomed, 0.25)).toBeCloseTo(before, 10);
  });

  it("holds it still through a chain of zooms, which is what a wheel produces", () => {
    let view = FULL_VIEW;
    const target = viewToSpan(view, 0.7);
    for (let i = 0; i < 8; i++) {
      view = zoomView(view, 0.7, 1.2);
    }
    expect(viewToSpan(view, 0.7)).toBeCloseTo(target, 10);
  });

  it("never leaves the span", () => {
    const edge = zoomView(FULL_VIEW, 0, 8);
    expect(edge.start).toBe(0);
    expect(viewWidth(edge)).toBeCloseTo(0.125, 10);
    const far = zoomView(FULL_VIEW, 1, 8);
    expect(far.end).toBe(1);
  });

  it("clamps out at full span and at the magnification floor", () => {
    expect(isFullView(zoomView(FULL_VIEW, 0.5, 0.25))).toBe(true);
    let view = FULL_VIEW;
    for (let i = 0; i < 100; i++) {
      view = zoomView(view, 0.5, 2);
    }
    expect(viewWidth(view)).toBeCloseTo(1 / 512, 10);
  });
});

describe("panView", () => {
  it("moves by the pointer's distance, whatever the zoom", () => {
    const view = zoomView(FULL_VIEW, 0.5, 4);
    const panned = panView(view, 0.5);
    expect(panned.start - view.start).toBeCloseTo(0.125, 10);
  });

  it("stops at the edge without changing the zoom level", () => {
    const view = zoomView(FULL_VIEW, 0.5, 4);
    const panned = panView(view, -5);
    expect(panned.start).toBe(0);
    expect(viewWidth(panned)).toBeCloseTo(viewWidth(view), 10);
  });
});

describe("view ↔ span mapping", () => {
  it("round-trips", () => {
    const view = zoomView(FULL_VIEW, 0.3, 6);
    expect(spanToView(view, viewToSpan(view, 0.42))).toBeCloseTo(0.42, 10);
  });

  it("reports off-screen positions rather than clamping them", () => {
    const view = { start: 0.4, end: 0.6 };
    expect(spanToView(view, 0.1)).toBeLessThan(0);
    expect(spanToView(view, 0.9)).toBeGreaterThan(1);
  });

  it("places a channel offset against the device span", () => {
    expect(offsetToSpan(0, 2_048_000)).toBe(0.5);
    expect(offsetToSpan(512_000, 2_048_000)).toBe(0.75);
  });
});

describe("niceStep", () => {
  it("walks the 1-2-5 ladder", () => {
    expect(niceStep(1)).toBe(1);
    expect(niceStep(1.4)).toBe(1);
    expect(niceStep(2.9)).toBe(2);
    expect(niceStep(6)).toBe(5);
    expect(niceStep(9)).toBe(10);
    expect(niceStep(230_000)).toBe(200_000);
  });

  it("survives a degenerate span instead of looping forever", () => {
    expect(niceStep(0)).toBe(1);
    expect(niceStep(Number.NaN)).toBe(1);
  });
});

describe("frequencyTicks", () => {
  it("lands on round frequencies inside the window", () => {
    const ticks = frequencyTicks(100e6, 2.048e6, FULL_VIEW, 6);
    expect(ticks.length).toBeGreaterThan(3);
    for (const tick of ticks) {
      expect(tick.hz % 500_000).toBe(0);
      expect(tick.at).toBeGreaterThanOrEqual(0);
      expect(tick.at).toBeLessThanOrEqual(1);
    }
  });

  it("refines as the view zooms in", () => {
    const wide = frequencyTicks(100e6, 2.048e6, FULL_VIEW, 6);
    const close = frequencyTicks(100e6, 2.048e6, zoomView(FULL_VIEW, 0.5, 16), 6);
    expect(tickGap(close)).toBeLessThan(tickGap(wide));
  });

  it("returns nothing for a span it cannot draw", () => {
    expect(frequencyTicks(100e6, 0, FULL_VIEW, 6)).toEqual([]);
  });
});

function tickGap(ticks: readonly { hz: number }[]): number {
  return (ticks[1]?.hz ?? 0) - (ticks[0]?.hz ?? 0);
}

describe("decibelTicks", () => {
  it("covers the frame's range on round values", () => {
    const ticks = decibelTicks(-91, -11, 4);
    expect(ticks.every((db) => db % 20 === 0)).toBe(true);
    expect(ticks[0]).toBeGreaterThanOrEqual(-91);
    expect(ticks.at(-1)).toBeLessThanOrEqual(-11);
  });

  it("returns nothing for an inverted range", () => {
    expect(decibelTicks(0, -10, 4)).toEqual([]);
  });
});
