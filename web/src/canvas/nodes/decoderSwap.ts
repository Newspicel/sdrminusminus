import type {
  ChannelDescriptor,
  ChannelInfo,
  ChannelSettings,
  PatchGraph,
  PatchNode,
  WorkspaceSnapshot,
} from "../../lib/types";
import { type GraphContext, nodeOf, patchNode, portsOf } from "../graph";
import { keepsCalls } from "./callRecording";

export const ANALOG_MODES = ["nfm", "wfm", "am", "ssb"] as const;

export function nextAnalogMode(current: string, direction: number): string {
  const at = ANALOG_MODES.indexOf(current as (typeof ANALOG_MODES)[number]);
  if (at < 0) {
    return ANALOG_MODES[0];
  }
  const length = ANALOG_MODES.length;
  return ANALOG_MODES[(at + direction + length) % length] ?? ANALOG_MODES[0];
}

export function retypeChannel(
  context: GraphContext,
  graph: PatchGraph,
  id: string,
  descriptor: ChannelDescriptor,
): PatchGraph {
  const retyped = patchNode(graph, id, (node) =>
    node.kind === "channel"
      ? {
          ...node,
          data: {
            ...node.data,
            channel_type: descriptor.type_id,
            ...(keepsCalls(descriptor) ? {} : { record_calls: false }),
          },
        }
      : node,
  );
  const node = nodeOf(retyped, id);
  if (node === undefined || node.kind !== "channel") {
    return retyped;
  }
  const kept = new Set(portsOf(context, retyped, node).map((port) => port.name));
  const held = retyped.edges ?? [];
  const edges = held.filter(
    (edge) =>
      (edge.from.node !== id || kept.has(edge.from.port)) &&
      (edge.to.node !== id || kept.has(edge.to.port)),
  );
  return edges.length === held.length ? retyped : { ...retyped, edges };
}

export function retypedSettings(
  current: ChannelSettings | null,
  descriptor: ChannelDescriptor,
): ChannelSettings | null {
  const defaults = descriptor.defaults;
  if (defaults == null) {
    return null;
  }
  if (current === null) {
    return defaults;
  }
  return {
    ...defaults,
    frequency_hz: current.frequency_hz,
    ...(current.squelch === undefined ? {} : { squelch: current.squelch }),
  };
}

export interface DecoderSwap {
  context: GraphContext;
  node: PatchNode;
  descriptor: ChannelDescriptor;
  live: { deviceSet: number; channel: ChannelInfo } | null;
  saved: ChannelSettings | null;
  applyEdit: (deviceSet: number, channel: number, settings: ChannelSettings) => void;
  saveChannel: (node: string, settings: ChannelSettings) => void;
  edit: (change: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
}

export function swapDecoder(swap: DecoderSwap): boolean {
  const { node, descriptor } = swap;
  if (node.kind !== "channel" || node.data.channel_type === descriptor.type_id) {
    return false;
  }
  const settings = retypedSettings(swap.live?.channel.settings ?? swap.saved, descriptor);
  if (settings === null) {
    return false;
  }
  if (swap.live === null) {
    swap.saveChannel(node.id, settings);
  } else {
    swap.applyEdit(swap.live.deviceSet, swap.live.channel.id, settings);
  }
  swap.edit((snapshot) => ({
    ...snapshot,
    graph: retypeChannel(swap.context, snapshot.graph, node.id, descriptor),
  }));
  return true;
}
