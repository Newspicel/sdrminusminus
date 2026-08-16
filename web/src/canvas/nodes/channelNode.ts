import type { DeviceInfo, DeviceRef, PatchGraph } from "../../lib/types";
import { deviceNodeOf, refMatches } from "../binding";

export type ChannelBinding =
  | "unwired"
  | "no-radio"
  | "radio-absent"
  | "radio-closed"
  | "not-started";

export function radioRefOf(graph: PatchGraph, node: string): DeviceRef | null {
  const device = deviceNodeOf(graph, node);
  const found = graph.nodes.find((candidate) => candidate.id === device);
  return found?.kind === "device" ? (found.data.device ?? null) : null;
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

const LABELS: Record<ChannelBinding, string> = {
  unwired: "no device",
  "no-radio": "no radio",
  "radio-absent": "radio disconnected",
  "radio-closed": "radio not open",
  "not-started": "not started",
};

const SAID: Record<ChannelBinding, string> = {
  unwired: "Nothing feeds this channel. Wire a device's IQ output into its input.",
  "no-radio": "The device node feeding this channel has no radio chosen yet. Pick one there.",
  "radio-absent":
    "Its radio is not connected. Plug it back in and open it — this channel starts with these settings as soon as the radio runs.",
  "radio-closed": "Its radio is plugged in but not open. Opening it starts this channel too.",
  "not-started": "The radio is running, but this channel has not started on it yet.",
};

const ACTIONS: Partial<Record<ChannelBinding, string>> = {
  "radio-closed": "Open radio",
  "not-started": "Start channel",
};

export function channelBindingLabel(binding: ChannelBinding): string {
  return LABELS[binding];
}

export function channelBindingSaid(binding: ChannelBinding): string {
  return SAID[binding];
}

export function channelBindingAction(binding: ChannelBinding): string | null {
  return ACTIONS[binding] ?? null;
}
