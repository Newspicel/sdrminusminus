import type { DeviceInfo, DeviceRef, PatchGraph } from "../../lib/types";
import { deviceNodeOf, nodeDeviceRef, refMatches } from "../binding";
import { tuningLocked } from "../graph";

export type ChannelBinding =
  | "unwired"
  | "no-radio"
  | "radio-absent"
  | "radio-closed"
  | "not-started";

export function radioRefOf(graph: PatchGraph, node: string): DeviceRef | null {
  const device = deviceNodeOf(graph, node);
  const found = graph.nodes.find((candidate) => candidate.id === device);
  return found === undefined ? null : nodeDeviceRef(found);
}

export function lockedChannels(
  graph: PatchGraph,
  faces: ReadonlyMap<number, string>,
): ReadonlySet<number> {
  const locked = new Set<number>();
  for (const [channel, node] of faces) {
    if (tuningLocked(graph, node)) {
      locked.add(channel);
    }
  }
  return locked;
}

export function radioIsAttached(
  reference: DeviceRef | null,
  attached: readonly DeviceInfo[],
): boolean {
  return reference !== null && attached.some((device) => refMatches(reference, device));
}

export function channelBinding(input: {
  wired: boolean;
  open: boolean;
  named: boolean;
  attached: boolean;
}): ChannelBinding {
  if (!input.wired) {
    return "unwired";
  }
  if (input.open) {
    return "not-started";
  }
  if (!input.named) {
    return "no-radio";
  }
  return input.attached ? "radio-closed" : "radio-absent";
}

const HINTS: Record<ChannelBinding, string> = {
  unwired: "Wire a device's IQ in",
  "no-radio": "Pick a radio on the device node",
  "radio-absent": "Its radio is not connected",
  "radio-closed": "Its radio is not open",
  "not-started": "Not started on the radio yet",
};

const ACTIONS: Partial<Record<ChannelBinding, string>> = {
  "radio-closed": "Open radio",
  "not-started": "Start channel",
};

const STATUS: Record<ChannelBinding, string> = {
  unwired: "unwired",
  "no-radio": "no radio",
  "radio-absent": "radio missing",
  "radio-closed": "radio closed",
  "not-started": "not started",
};

export function channelBindingStatus(binding: ChannelBinding): string {
  return STATUS[binding];
}

export function channelBindingHint(binding: ChannelBinding): string {
  return HINTS[binding];
}

export function channelBindingAction(binding: ChannelBinding): string | null {
  return ACTIONS[binding] ?? null;
}
