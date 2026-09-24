import type { ChannelInfo, DeviceSet } from "../lib/types";
import { controlledNodeOf, deviceNodeOf, type IqLane, iqSourceOf } from "./binding";
import type { Workspace } from "./context";

export function deviceSetOf(workspace: Workspace, node: string): DeviceSet | null {
  const owner = deviceNodeOf(workspace.graph, node, workspace.owners);
  return owner === null ? null : (workspace.devices.get(owner) ?? null);
}

export function laneOf(workspace: Workspace, node: string): IqLane | null {
  const wired = iqSourceOf(workspace.graph, node, workspace.devices);
  const owner = workspace.owners.get(node);
  const channel = workspace.channels.get(node);
  if (owner !== undefined && channel !== undefined) {
    const stream = channel.stream ?? 0;
    return wired?.source === owner && wired.stream === stream ? wired : { source: owner, stream };
  }
  return wired;
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
