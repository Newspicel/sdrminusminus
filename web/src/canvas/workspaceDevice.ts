import type { ChannelInfo, DeviceSet } from "../lib/types";
import { controlledNodeOf, deviceNodeOf } from "./binding";
import type { Workspace } from "./context";

export function deviceSetOf(workspace: Workspace, node: string): DeviceSet | null {
  const owner = deviceNodeOf(workspace.graph, node);
  return owner === null ? null : (workspace.devices.get(owner) ?? null);
}

export interface Decoder {
  node: string;
  set: DeviceSet;
  channel: ChannelInfo;
}

export function decoderOf(workspace: Workspace, node: string): Decoder | null {
  const driven = controlledNodeOf(workspace.graph, node);
  if (driven === null) {
    return null;
  }
  const channel = workspace.channels.get(driven);
  const set = deviceSetOf(workspace, driven);
  return channel === undefined || set === null ? null : { node: driven, set, channel };
}
