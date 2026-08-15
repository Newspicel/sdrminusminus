import { describe, expect, it } from "vitest";
import { dropPosition, type PlacementRect } from "./placement";

const viewport = { x: 1_800, y: -600, w: 1_200, h: 800 };
const size = { w: 300, h: 200 };

function overlaps(position: { x: number; y: number }, occupied: PlacementRect): boolean {
  return (
    position.x < occupied.x + occupied.w &&
    position.x + size.w > occupied.x &&
    position.y < occupied.y + occupied.h &&
    position.y + size.h > occupied.y
  );
}

describe("dropPosition", () => {
  it("centres the first node in the visible flow coordinates", () => {
    expect(dropPosition(viewport, size, [], 20)).toEqual({ x: 2_250, y: -300 });
  });

  it("chooses the nearest clear visible position deterministically", () => {
    const occupied = [
      { x: 2_200, y: -350, w: 400, h: 300 },
      { x: 1_820, y: -580, w: 300, h: 200 },
    ];
    const first = dropPosition(viewport, size, occupied, 20);
    const second = dropPosition(viewport, size, occupied, 20);

    expect(second).toEqual(first);
    expect(first.x).toBeGreaterThanOrEqual(viewport.x);
    expect(first.y).toBeGreaterThanOrEqual(viewport.y);
    expect(first.x + size.w).toBeLessThanOrEqual(viewport.x + viewport.w);
    expect(first.y + size.h).toBeLessThanOrEqual(viewport.y + viewport.h);
    expect(occupied.every((rect) => !overlaps(first, rect))).toBe(true);
  });

  it("uses an edge of an existing node to find a narrow clear lane", () => {
    const occupied = [
      { x: 1_800, y: -600, w: 1_200, h: 250 },
      { x: 1_800, y: -100, w: 1_200, h: 300 },
    ];
    const position = dropPosition(viewport, size, occupied, 20);

    expect(position.y).toBe(-320);
    expect(occupied.every((rect) => !overlaps(position, rect))).toBe(true);
  });

  it("places a node clear of the others when the visible area is full", () => {
    const occupied = [{ x: viewport.x, y: viewport.y, w: viewport.w, h: viewport.h }];
    const first = dropPosition(viewport, size, occupied, 20);
    const second = dropPosition(viewport, size, occupied, 20);

    expect(second).toEqual(first);
    expect(occupied.every((rect) => !overlaps(first, rect))).toBe(true);
  });

  it("stays beside the view rather than across the patch when it has to widen", () => {
    const occupied = [
      { x: viewport.x, y: viewport.y, w: viewport.w, h: viewport.h },
      { x: 20_000, y: 20_000, w: 400, h: 400 },
    ];
    const position = dropPosition(viewport, size, occupied, 20);

    expect(position.x).toBeLessThan(viewport.x + viewport.w + size.w);
    expect(position.y).toBeLessThan(viewport.y + viewport.h + size.h);
    expect(occupied.every((rect) => !overlaps(position, rect))).toBe(true);
  });
});
