import type { PatchGraph } from "../../lib/types";
import { hasWire } from "../binding";
import { useWorkspaceContext } from "../context";

export const RADIO_IDLE =
  "Wired up, but the radio it comes from is not open. Open it on its device node.";
export const CHANNEL_IDLE =
  "Wired up, but the channel it comes from is not running. Start it on its node, or open its radio first.";

export function faceEmptyText(
  graph: PatchGraph,
  node: string,
  port: string,
  unwired: string,
): string {
  if (!hasWire(graph, node, port)) {
    return unwired;
  }
  return port === "iq" ? RADIO_IDLE : CHANNEL_IDLE;
}

export function useFaceEmptyText(node: string, port: string, unwired: string): string {
  return faceEmptyText(useWorkspaceContext().graph, node, port, unwired);
}
