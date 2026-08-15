import { type Node, type Rect, useReactFlow, useStoreApi } from "@xyflow/react";
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

export function useNodePlacement(): (
  graph: PatchGraph,
  kind: NodeKind,
  size?: { w: number; h: number },
) => Position {
  const flow = useReactFlow();
  const store = useStoreApi();

  return useCallback(
    (graph, kind, requested) => {
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
      const size = requested ?? NODE_SIZE[kind];

      if (width <= 0 || height <= 0 || zoom <= 0) {
        return dropPosition({ x: 0, y: 0, w: 1200, h: 800 }, size, occupied, SCREEN_GAP);
      }
      const position = dropPosition(viewport, size, occupied, gap);
      const face = {
        x: position.x - gap,
        y: position.y - gap,
        width: size.w + 2 * gap,
        height: size.h + 2 * gap,
      };
      if (!encloses(viewport, face)) {
        void flow.fitBounds(union(viewport, face), { padding: 0 });
      }
      return position;
    },
    [flow, store],
  );
}

export function dropPosition(
  viewport: PlacementRect,
  size: { w: number; h: number },
  occupied: readonly PlacementRect[],
  gap = SCREEN_GAP,
): Position {
  const centre = { x: viewport.x + viewport.w / 2, y: viewport.y + viewport.h / 2 };
  return (
    clearPosition(viewport, size, occupied, gap, centre) ??
    clearPosition(widened(viewport, size, occupied, gap), size, occupied, gap, centre) ?? {
      x: viewport.x,
      y: viewport.y,
    }
  );
}

function clearPosition(
  area: PlacementRect,
  size: { w: number; h: number },
  occupied: readonly PlacementRect[],
  gap: number,
  centre: Position,
): Position | undefined {
  const nearby = occupied.filter(
    (rect) =>
      rect.x < area.x + area.w + gap &&
      rect.x + rect.w + gap > area.x &&
      rect.y < area.y + area.h + gap &&
      rect.y + rect.h + gap > area.y,
  );
  const xs = axisCandidates(area.x, area.w, size.w, nearby, "x", "w", gap);
  const ys = axisCandidates(area.y, area.h, size.h, nearby, "y", "h", gap);
  return xs
    .flatMap((x) => ys.map((y) => ({ x, y })))
    .filter((candidate) => nearby.every((rect) => !intersects(candidate, size, rect, gap)))
    .toSorted(
      (a, b) => distance(a, size, centre) - distance(b, size, centre) || a.y - b.y || a.x - b.x,
    )[0];
}

function widened(
  viewport: PlacementRect,
  size: { w: number; h: number },
  occupied: readonly PlacementRect[],
  gap: number,
): PlacementRect {
  const margin = { x: size.w + 2 * gap, y: size.h + 2 * gap };
  const left = Math.min(viewport.x, ...occupied.map((rect) => rect.x)) - margin.x;
  const top = Math.min(viewport.y, ...occupied.map((rect) => rect.y)) - margin.y;
  const right =
    Math.max(viewport.x + viewport.w, ...occupied.map((rect) => rect.x + rect.w)) + margin.x;
  const bottom =
    Math.max(viewport.y + viewport.h, ...occupied.map((rect) => rect.y + rect.h)) + margin.y;
  return { x: left, y: top, w: right - left, h: bottom - top };
}

function encloses(viewport: PlacementRect, face: Rect): boolean {
  return (
    face.x >= viewport.x &&
    face.y >= viewport.y &&
    face.x + face.width <= viewport.x + viewport.w &&
    face.y + face.height <= viewport.y + viewport.h
  );
}

function union(viewport: PlacementRect, face: Rect): Rect {
  const x = Math.min(viewport.x, face.x);
  const y = Math.min(viewport.y, face.y);
  return {
    x,
    y,
    width: Math.max(viewport.x + viewport.w, face.x + face.width) - x,
    height: Math.max(viewport.y + viewport.h, face.y + face.height) - y,
  };
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

function distance(position: Position, size: { w: number; h: number }, centre: Position): number {
  const dx = position.x + size.w / 2 - centre.x;
  const dy = position.y + size.h / 2 - centre.y;
  return dx * dx + dy * dy;
}
