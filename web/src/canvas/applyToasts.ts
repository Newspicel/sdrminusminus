import type { PatchApplyReport, PatchNode } from "../lib/types";

function named(nodes: readonly PatchNode[], id: string): string {
  const node = nodes.find((candidate) => candidate.id === id);
  return node?.label ?? (node?.kind === "channel" ? node.data.channel_type.toUpperCase() : id);
}

export function applyToasts(
  report: PatchApplyReport | null,
  nodes: readonly PatchNode[],
): readonly string[] {
  if (report === null) {
    return [];
  }
  return [
    ...(report.refused ?? []).map((refusal) => `${named(nodes, refusal.node)}: ${refusal.reason}`),
    ...(report.absent ?? []).map(
      (node) => `${named(nodes, node)}: its radio is not connected, so nothing on it was started`,
    ),
  ];
}
