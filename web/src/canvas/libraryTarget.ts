import type { DeviceSet, PatchGraph } from "../lib/types";
import { deviceNodeOf } from "./binding";
import { nodeOf, tuningLocked } from "./graph";

export interface DeviceTarget {
  kind: "device";
  node: string;
  set: DeviceSet;
  locked: boolean;
}

export interface ChannelTarget {
  kind: "channel";
  node: string;
  set: DeviceSet | null;
  locked: boolean;
}

export type TuneTarget = DeviceTarget | ChannelTarget;

export function libraryTarget(
  graph: PatchGraph,
  devices: ReadonlyMap<string, DeviceSet>,
  selected: string | null,
  owners: ReadonlyMap<string, string> = new Map(),
): TuneTarget | null {
  if (selected !== null) {
    const set = devices.get(selected);
    if (set !== undefined) {
      return { kind: "device", node: selected, set, locked: tuningLocked(graph, selected) };
    }
    if (nodeOf(graph, selected)?.kind === "channel") {
      const owner = deviceNodeOf(graph, selected, owners);
      return {
        kind: "channel",
        node: selected,
        set: owner === null ? null : (devices.get(owner) ?? null),
        locked: tuningLocked(graph, selected),
      };
    }
  }
  const only = devices.size === 1 ? [...devices.entries()][0] : undefined;
  return only === undefined
    ? null
    : { kind: "device", node: only[0], set: only[1], locked: tuningLocked(graph, only[0]) };
}
