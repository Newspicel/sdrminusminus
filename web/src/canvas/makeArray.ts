import type { DeviceSet, NodeBodyOf, PatchGraph, PatchNode, Position } from "../lib/types";
import { arrayHoldingLane, arrayLabel } from "./arrayRules";
import {
  ARRAY_LANE_PORT,
  addEdge,
  addNode,
  NODE_SIZE,
  nodeOf,
  rxStreamCount,
  streamPort,
} from "./graph";

export type MakeArrayCheck = { ok: true; lanes: number } | { ok: false; reason: string };

export const OPEN_FIRST = "Open the radio first";
export const ONE_LANE = "One lane only";
export const NO_SHARED_CLOCK = "Lanes share no clock";
export const MAKE_ARRAY_TITLE = "New Array wired to every lane";

const ARRAY_GAP_PX = 80;

export function canMakeArray(
  graph: PatchGraph,
  device: string,
  set: DeviceSet | undefined,
): MakeArrayCheck {
  if (set === undefined) {
    return { ok: false, reason: OPEN_FIRST };
  }
  const lanes = rxStreamCount(set.capabilities);
  if (lanes < 2) {
    return { ok: false, reason: ONE_LANE };
  }
  if ((set.capabilities.coherence ?? "none") === "none") {
    return { ok: false, reason: NO_SHARED_CLOCK };
  }
  for (let stream = 0; stream < lanes; stream++) {
    const holder = arrayHoldingLane(graph, { node: device, port: streamPort("iq", stream) });
    if (holder !== null) {
      return { ok: false, reason: `In ${arrayLabel(graph, holder)}` };
    }
  }
  return { ok: true, lanes };
}

export function arrayPlacement(graph: PatchGraph, device: string): Position {
  const position = nodeOf(graph, device)?.position ?? { x: 0, y: 0 };
  return { x: position.x + NODE_SIZE.device.w + ARRAY_GAP_PX, y: position.y };
}

export function makeArray(
  graph: PatchGraph,
  device: string,
  lanes: number,
  body: NodeBodyOf<"array">,
  id: string,
): PatchGraph {
  const array: PatchNode = { id, position: arrayPlacement(graph, device), ...body };
  let next = addNode(graph, array);
  for (let stream = 0; stream < lanes; stream++) {
    next = addEdge(next, {
      from: { node: device, port: streamPort("iq", stream) },
      to: { node: id, port: streamPort(ARRAY_LANE_PORT, stream) },
    });
  }
  return next;
}
