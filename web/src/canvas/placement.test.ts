import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PatchGraph, PatchNode } from "../lib/types";
import { NODE_SIZE } from "./graph";
import { dropPosition, type PlacementRect, useNodePlacement } from "./placement";

const flow = vi.hoisted(() => ({
  fitBounds: vi.fn(),
  getNodes: vi.fn(() => []),
  getViewport: vi.fn(() => ({ x: 0, y: 0, zoom: 1 })),
}));
const store = vi.hoisted(() => ({
  getState: vi.fn(() => ({ width: 1_200, height: 800 })),
}));

vi.mock("@xyflow/react", () => ({
  useReactFlow: () => flow,
  useStoreApi: () => store,
}));
vi.mock("react", () => ({ useCallback: (callback: unknown) => callback }));

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

function node(id: string, body: Partial<PatchNode> & Pick<PatchNode, "kind">): PatchNode {
  return { id, position: { x: 0, y: 0 }, ...body } as PatchNode;
}

beforeEach(() => {
  flow.fitBounds.mockClear();
  flow.getNodes.mockClear();
  flow.getViewport.mockClear();
  store.getState.mockClear();
});

describe("useNodePlacement", () => {
  it("fits the viewport and a new node when the visible graph is crowded", () => {
    const graph: PatchGraph = {
      nodes: [node("crowded", { kind: "scope", size: { w: 1_200, h: 800 } })],
      edges: [],
    };
    const position = useNodePlacement()(graph, "channel");

    expect(flow.fitBounds).toHaveBeenCalledTimes(1);
    const [bounds, options] = flow.fitBounds.mock.calls[0] ?? [];
    expect(options).toEqual({ padding: 0.12 });
    expect(bounds.x).toBeLessThanOrEqual(0);
    expect(bounds.y).toBeLessThanOrEqual(0);
    expect(bounds.x + bounds.width).toBeGreaterThanOrEqual(1_200);
    expect(bounds.y + bounds.height).toBeGreaterThanOrEqual(800);
    expect(bounds.x).toBeLessThanOrEqual(position.x);
    expect(bounds.y).toBeLessThanOrEqual(position.y);
    expect(bounds.x + bounds.width).toBeGreaterThanOrEqual(position.x + NODE_SIZE.channel.w);
    expect(bounds.y + bounds.height).toBeGreaterThanOrEqual(position.y + NODE_SIZE.channel.h);
  });
});

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

  it("places a node just beyond a crowded viewport instead of overlapping", () => {
    const occupied = [{ x: viewport.x, y: viewport.y, w: viewport.w, h: viewport.h }];
    const position = dropPosition(viewport, size, occupied, 20);

    expect(occupied.every((rect) => !overlaps(position, rect))).toBe(true);
    expect(
      position.x < viewport.x ||
        position.y < viewport.y ||
        position.x + size.w > viewport.x + viewport.w ||
        position.y + size.h > viewport.y + viewport.h,
    ).toBe(true);
  });

  it("finds a clear position for the fixed-size starter patch", () => {
    const starterViewport = { x: -170, y: -52, w: 1_280, h: 684 };
    const channel = { w: 440, h: 300 };
    const occupied = [
      { x: 0, y: 0, w: 380, h: 420 },
      { x: 420, y: 0, w: 520, h: 360 },
      { x: 420, y: 380, w: 320, h: 200 },
    ];
    const position = dropPosition(starterViewport, channel, occupied, 24);

    expect(
      occupied.every(
        (rect) =>
          position.x + channel.w <= rect.x ||
          position.x >= rect.x + rect.w ||
          position.y + channel.h <= rect.y ||
          position.y >= rect.y + rect.h,
      ),
    ).toBe(true);
  });
});
