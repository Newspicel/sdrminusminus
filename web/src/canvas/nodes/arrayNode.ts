import type { Coherence, DeviceRef, PatchGraph } from "../../lib/types";
import { streamPort } from "../graph";

export const TIER_NOTE: Record<Coherence, string> = {
  none: "",
  time_sync:
    "Delay between elements is meaningful, so passive radar works. Every retune scrambles the phase between separate radios, so bearings need a pilot or an injected noise source for the calibration to solve against.",
  phase_coherent:
    "The radios share a synthesizer as well as a clock, so phase between elements survives a retune and bearings are meaningful.",
};

export interface ArrayMember {
  /// The device node feeding this element.
  node: string;
  device: DeviceRef | null;
}

/// The radios wired into an array, in the order of the inputs they arrive on. That order is the
/// element numbering, so re-wiring is how an operator corrects an array they cabled out of order.
export function arrayMembers(graph: PatchGraph, node: string): ArrayMember[] {
  const found: ArrayMember[] = [];
  const entry = graph.nodes.find((candidate) => candidate.id === node);
  const wanted = entry?.kind === "array" ? entry.data.members + 1 : 0;
  for (let element = 0; element < wanted; element++) {
    const port = streamPort("iq", element);
    const wire = (graph.edges ?? []).find((edge) => edge.to.node === node && edge.to.port === port);
    if (wire === undefined) {
      continue;
    }
    const source = graph.nodes.find((candidate) => candidate.id === wire.from.node);
    found.push({
      node: wire.from.node,
      device: source?.kind === "device" ? (source.data.device ?? null) : null,
    });
  }
  return found;
}

/// Which array has taken a radio, if one has. A radio in an array is opened and tuned by it.
export function arrayHolding(graph: PatchGraph, deviceNode: string): string | null {
  for (const node of graph.nodes) {
    if (node.kind !== "array") {
      continue;
    }
    if (arrayMembers(graph, node.id).some((member) => member.node === deviceNode)) {
      return node.id;
    }
  }
  return null;
}

export function arrayKey(nodeId: string): string {
  return nodeId.replaceAll(":", "-");
}
