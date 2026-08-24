import { describe, expect, it } from "vitest";
import { boxScrolls, movesCanvas, verticalWheel, type WheelBox } from "./wheel";

const wheel = (over: Partial<WheelBox> = {}): WheelBox => ({
  overflowX: "visible",
  overflowY: "visible",
  scrollWidth: 100,
  clientWidth: 100,
  scrollHeight: 100,
  clientHeight: 100,
  ...over,
});

const gesture = (over: Partial<Parameters<typeof movesCanvas>[0]> = {}) => ({
  ctrlKey: false,
  metaKey: false,
  deltaX: 0,
  deltaY: 40,
  ...over,
});

describe("movesCanvas", () => {
  it("keeps a plain wheel out of the canvas gestures", () => {
    expect(movesCanvas(gesture())).toBe(false);
  });

  it("claims the pinch gesture", () => {
    expect(movesCanvas(gesture({ ctrlKey: true }))).toBe(true);
  });

  it("claims the zoom modifier", () => {
    expect(movesCanvas(gesture({ metaKey: true }))).toBe(true);
  });
});

describe("verticalWheel", () => {
  it("reads a straight scroll as vertical", () => {
    expect(verticalWheel({ deltaX: 0, deltaY: 40 })).toBe(true);
  });

  it("reads a sideways swipe as horizontal", () => {
    expect(verticalWheel({ deltaX: -40, deltaY: 3 })).toBe(false);
  });

  it("breaks a tie towards vertical", () => {
    expect(verticalWheel({ deltaX: 20, deltaY: 20 })).toBe(true);
  });
});

describe("boxScrolls", () => {
  it("leaves an unscrollable box alone", () => {
    expect(boxScrolls(wheel(), gesture())).toBe(false);
  });

  it("takes a wheel a scrollable column can use", () => {
    expect(boxScrolls(wheel({ overflowY: "auto", scrollHeight: 400 }), gesture())).toBe(true);
  });

  it("leaves a column that overflows without scrolling", () => {
    expect(boxScrolls(wheel({ overflowY: "hidden", scrollHeight: 400 }), gesture())).toBe(false);
  });

  it("leaves a scroller with nothing to scroll", () => {
    expect(boxScrolls(wheel({ overflowY: "auto" }), gesture())).toBe(false);
  });

  it("routes a sideways wheel by the horizontal axis", () => {
    const box = wheel({ overflowX: "auto", scrollWidth: 400, overflowY: "auto" });
    expect(boxScrolls(box, gesture({ deltaX: -40, deltaY: 0 }))).toBe(true);
    expect(boxScrolls(wheel({ overflowX: "auto", scrollWidth: 400 }), gesture())).toBe(false);
  });

  it("prefers the vertical axis on a diagonal wheel", () => {
    const box = wheel({ overflowX: "auto", scrollWidth: 400 });
    expect(boxScrolls(box, gesture({ deltaX: 10, deltaY: 40 }))).toBe(false);
  });

  it("takes a scroll box that scrolls in either direction", () => {
    expect(boxScrolls(wheel({ overflowY: "scroll", scrollHeight: 400 }), gesture())).toBe(true);
  });
});
