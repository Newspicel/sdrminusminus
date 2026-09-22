import { describe, expect, it } from "vitest";
import { ReadoutHold } from "./readoutHold";

describe("ReadoutHold", () => {
  it("shows a new bin at once", () => {
    const hold = new ReadoutHold();
    expect(hold.read(3, -70, 0)).toBe(-70);
    expect(hold.read(4, -40, 16)).toBe(-40);
  });

  it("holds the shown level between refreshes", () => {
    const hold = new ReadoutHold();
    hold.read(3, -70, 0);
    expect(hold.read(3, -50, 16)).toBe(-70);
    expect(hold.read(3, -90, 200)).toBe(-70);
  });

  it("settles on the mean power of a noisy bin", () => {
    const hold = new ReadoutHold();
    let shown = hold.read(3, -80, 0);
    for (let now = 16; now < 5000; now += 16) {
      shown = hold.read(3, now % 32 === 0 ? -77 : -83, now);
    }
    expect(shown).toBeGreaterThan(-80);
    expect(shown).toBeLessThan(-78.5);
  });

  it("passes an empty level through", () => {
    const hold = new ReadoutHold();
    expect(hold.read(3, Number.NEGATIVE_INFINITY, 0)).toBe(Number.NEGATIVE_INFINITY);
  });
});
