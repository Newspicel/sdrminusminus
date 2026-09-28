import { describe, expect, it } from "vitest";
import { patchLayout, route } from "./patch";

describe("route", () => {
  it("draws a straight wire between ports at one height", () => {
    expect(route({ x: 150, y: 172 }, { x: 210, y: 172 })).toBe("M150 172 H210");
  });

  it("bends, then runs flat to a far port", () => {
    expect(route({ x: 150, y: 172 }, { x: 420, y: 302 }, 270)).toBe(
      "M150 172 C210 172 210 302 270 302 H420",
    );
  });
});

describe("patchLayout", () => {
  it("keeps the wide layout as drawn before", () => {
    const wide = patchLayout(false);
    expect(wide.viewBox).toBe("-8 24 576 340");
    expect(wide.wires.find((wire) => wire.id === "tune")?.d).toBe("M150 172 H210");
  });

  it("fits a phone without shrinking the text", () => {
    const narrow = patchLayout(true);
    expect(narrow.width).toBeLessThanOrEqual(360);
    for (const node of narrow.nodes) {
      expect(node.x + node.w).toBeLessThanOrEqual(narrow.width);
    }
  });

  it("ends every wire on a port", () => {
    for (const layout of [patchLayout(false), patchLayout(true)]) {
      const ends = layout.ports.flatMap((port) => [`H${port.x}`, ` ${port.x} ${port.y}`]);
      for (const wire of layout.wires) {
        expect(ends.some((end) => wire.d.endsWith(end))).toBe(true);
      }
    }
  });
});
