import type { DeviceSet, PatchGraph } from "../../lib/types";
import { channelNodesOf } from "../binding";
import { tuningLocked } from "../graph";

export interface FollowScope {
  graph: PatchGraph;
  devices: ReadonlyMap<string, DeviceSet>;
  owners: ReadonlyMap<string, string>;
  selected: string | null;
}

export function followedDecoder(
  scope: FollowScope,
  deviceNode: string,
  stream: number,
): string | null {
  const wired = channelNodesOf(scope.graph, deviceNode, scope.devices)
    .filter((entry) => entry.stream === stream)
    .map((entry) => entry.node.id)
    .filter((id) => (scope.owners.get(id) ?? deviceNode) === deviceNode)
    .filter((id) => !tuningLocked(scope.graph, id));
  if (scope.selected !== null && wired.includes(scope.selected)) {
    return scope.selected;
  }
  return wired.length === 1 ? (wired[0] ?? null) : null;
}
