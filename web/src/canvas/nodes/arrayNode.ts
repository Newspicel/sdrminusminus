import type { PatchGraph } from "../../lib/types";
import { ARRAY_LANE_PORT, nodeOf, portStream } from "../graph";

export const ARRAY_NAME = "Array";

export function heldLanes(graph: PatchGraph, device: string): ReadonlyMap<number, string> {
  const held = new Map<number, string>();
  for (const edge of graph.edges ?? []) {
    const stream = edge.from.node === device ? portStream("iq", edge.from.port) : null;
    if (
      stream !== null &&
      portStream(ARRAY_LANE_PORT, edge.to.port) !== null &&
      nodeOf(graph, edge.to.node)?.kind === "array" &&
      !held.has(stream)
    ) {
      held.set(stream, edge.to.node);
    }
  }
  return held;
}

export function arrayName(graph: PatchGraph, array: string): string {
  return nodeOf(graph, array)?.label ?? ARRAY_NAME;
}

export interface LaneHold {
  array: string;
  label: string;
}

export function laneHolds(graph: PatchGraph, device: string): ReadonlyMap<number, LaneHold> {
  return new Map(
    [...heldLanes(graph, device)].map(([stream, array]) => [
      stream,
      { array, label: arrayName(graph, array) },
    ]),
  );
}

export function dialHold(
  holds: ReadonlyMap<number, LaneHold>,
  stream: number,
  merged: boolean,
): LaneHold | null {
  return (merged ? holds.get(stream) : holds.values().next().value) ?? null;
}
