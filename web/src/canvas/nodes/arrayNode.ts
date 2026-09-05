import type { DeviceRef, PatchGraph } from "../../lib/types";
import { streamPort } from "../graph";

export interface ArrayMember {
  node: string;
  device: DeviceRef | null;
}

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
