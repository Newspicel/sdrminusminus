import { type Node, useReactFlow, useStoreApi } from "@xyflow/react";
import { useCallback } from "react";
import type { NodeKind, PatchGraph, Position } from "../lib/types";
import { NODE_SIZE } from "./graph";

export interface PlacementRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

const SCREEN_GAP = 24;

/** Place new nodes against the camera the operator is looking through, using React Flow's last
 * measured face sizes when they are available. The provider outlives the patch/rack switch, so
 * the last patch viewport remains the target when a recording is opened from the rack. */
export function useNodePlacement(): (graph: PatchGraph, kind: NodeKind) => Position {
  const flow = useReactFlow();
  const store = useStoreApi();

  return useCallback(
    (graph, kind) => {
      const { width, height } = store.getState();
      const { x, y, zoom } = flow.getViewport();
      const gap = SCREEN_GAP / zoom;
      const viewport = {
        x: -x / zoom,
        y: -y / zoom,
        w: width / zoom,
        h: height / zoom,
      };
      const rendered = new Map(flow.getNodes().map((node) => [node.id, node]));
      const occupied = graph.nodes.map((node) => nodeRect(node, rendered.get(node.id)));
      const size = NODE_SIZE[kind];

      // Width is zero only before React Flow has mounted for the first time. The top bar cannot
      // be clicked in that interval, but keeping a deterministic fallback makes the helper safe
      // in tests and during a future server-rendered shell.
      if (width <= 0 || height <= 0 || zoom <= 0) {
        return dropPosition({ x: 0, y: 0, w: 1200, h: 800 }, size, occupied, SCREEN_GAP);
      }
      return dropPosition(viewport, size, occupied, gap);
    },
    [flow, store],
  );
}

/** Find the clear visible position nearest the centre. When a crowded viewport has no clear
 * footprint, keep the node visible and choose the candidate with the least overlap. */
export function dropPosition(
  viewport: PlacementRect,
  size: { w: number; h: number },
  occupied: readonly PlacementRect[],
  gap = SCREEN_GAP,
): Position {
  const nearby = occupied.filter(
    (rect) =>
      rect.x < viewport.x + viewport.w + gap &&
      rect.x + rect.w + gap > viewport.x &&
      rect.y < viewport.y + viewport.h + gap &&
      rect.y + rect.h + gap > viewport.y,
  );
  const xs = axisCandidates(viewport.x, viewport.w, size.w, nearby, "x", "w", gap);
  const ys = axisCandidates(viewport.y, viewport.h, size.h, nearby, "y", "h", gap);
  const centre = { x: viewport.x + viewport.w / 2, y: viewport.y + viewport.h / 2 };
  const candidates = xs
    .flatMap((x) => ys.map((y) => ({ x, y })))
    .toSorted(
      (a, b) => distance(a, size, centre) - distance(b, size, centre) || a.y - b.y || a.x - b.x,
    );
  const clear = candidates.find((candidate) =>
    nearby.every((rect) => !intersects(candidate, size, rect, gap)),
  );
  if (clear !== undefined) {
    return clear;
  }
  return (
    candidates.toSorted(
      (a, b) =>
        overlap(a, size, nearby) - overlap(b, size, nearby) ||
        distance(a, size, centre) - distance(b, size, centre) ||
        a.y - b.y ||
        a.x - b.x,
    )[0] ?? { x: viewport.x, y: viewport.y }
  );
}

function nodeRect(node: PatchGraph["nodes"][number], rendered: Node | undefined): PlacementRect {
  const measured = measuredSize(rendered);
  const natural = NODE_SIZE[node.kind];
  const stored = node.size;
  return {
    x: rendered?.position.x ?? node.position.x,
    y: rendered?.position.y ?? node.position.y,
    w: measured?.w ?? stored?.w ?? natural.w,
    h: measured?.h ?? stored?.h ?? natural.h,
  };
}

function measuredSize(node: Node | undefined): { w: number; h: number } | undefined {
  const w = node?.measured?.width ?? node?.width;
  const h = node?.measured?.height ?? node?.height;
  return w == null || h == null ? undefined : { w, h };
}

function axisCandidates(
  start: number,
  span: number,
  size: number,
  occupied: readonly PlacementRect[],
  position: "x" | "y",
  extent: "w" | "h",
  gap: number,
): number[] {
  const inset = Math.min(gap, Math.max(0, (span - size) / 2));
  const min = start + inset;
  const end = start + span - size - inset;
  const max = Math.max(min, end);
  const clamp = (value: number) => Math.min(Math.max(value, min), max);
  const values = [start + (span - size) / 2, min, max];
  for (const rect of occupied) {
    values.push(rect[position], rect[position] - size - gap, rect[position] + rect[extent] + gap);
  }
  return [...new Set(values.map(clamp))];
}

function intersects(
  position: Position,
  size: { w: number; h: number },
  rect: PlacementRect,
  gap: number,
): boolean {
  return (
    position.x < rect.x + rect.w + gap &&
    position.x + size.w + gap > rect.x &&
    position.y < rect.y + rect.h + gap &&
    position.y + size.h + gap > rect.y
  );
}

function overlap(
  position: Position,
  size: { w: number; h: number },
  occupied: readonly PlacementRect[],
): number {
  return occupied.reduce((area, rect) => {
    const w = Math.max(
      0,
      Math.min(position.x + size.w, rect.x + rect.w) - Math.max(position.x, rect.x),
    );
    const h = Math.max(
      0,
      Math.min(position.y + size.h, rect.y + rect.h) - Math.max(position.y, rect.y),
    );
    return area + w * h;
  }, 0);
}

function distance(position: Position, size: { w: number; h: number }, centre: Position): number {
  const dx = position.x + size.w / 2 - centre.x;
  const dy = position.y + size.h / 2 - centre.y;
  return dx * dx + dy * dy;
}
