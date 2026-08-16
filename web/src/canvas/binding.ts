import type {
  ChannelInfo,
  DeviceInfo,
  DeviceRef,
  DeviceSet,
  EventFilterNode,
  PatchGraph,
  PatchNodeOf,
  TrunkSystemStatus,
} from "../lib/types";

export interface Input {
  node: string;
  deviceSet: number;
  channel: ChannelInfo;
}

import { portStream } from "./graph";
import type { WiredSource } from "./nodes/eventFilter";

export function deviceRefOf(info: DeviceInfo): DeviceRef {
  return {
    backend: info.driver,
    ...(info.serial == null ? {} : { serial: info.serial }),
    ...(info.serial == null || info.key.startsWith(`${info.serial}@`) ? { key: info.key } : {}),
  };
}

export function refFromDeviceId(id: string): DeviceRef | null {
  const at = id.indexOf(":");
  if (at <= 0 || at === id.length - 1) {
    return null;
  }
  return { backend: id.slice(0, at), key: id.slice(at + 1) };
}

export function refMatches(reference: DeviceRef, info: DeviceInfo): boolean {
  if (reference.backend !== info.driver) {
    return false;
  }
  if (reference.serial != null) {
    return (
      reference.serial === info.serial && (reference.key == null || reference.key === info.key)
    );
  }
  return reference.key == null || reference.key === info.key;
}

export function claimedDevices(graph: PatchGraph, exceptNode: string): DeviceRef[] {
  const claimed: DeviceRef[] = [];
  for (const node of graph.nodes) {
    if (node.kind === "device" && node.id !== exceptNode && node.data.device != null) {
      claimed.push(node.data.device);
    }
  }
  return claimed;
}

export function bindDevices(graph: PatchGraph, sets: readonly DeviceSet[]): Map<string, DeviceSet> {
  const bound = new Map<string, DeviceSet>();
  const claimed = new Set<number>();
  for (const node of graph.nodes) {
    if (node.kind !== "device" || node.data.device == null) {
      continue;
    }
    const reference = node.data.device;
    const set = sets.find(
      (candidate) => !claimed.has(candidate.id) && refMatches(reference, candidate.device),
    );
    if (set !== undefined) {
      claimed.add(set.id);
      bound.set(node.id, set);
    }
  }
  return bound;
}

export function bindChannels(
  graph: PatchGraph,
  devices: ReadonlyMap<string, DeviceSet>,
): Map<string, ChannelInfo> {
  const bound = new Map<string, ChannelInfo>();
  for (const [deviceNode, set] of devices) {
    const free = [...set.channels];
    for (const { node, stream } of channelNodesOf(graph, deviceNode)) {
      const at = free.findIndex(
        (channel) =>
          channel.settings.params.type === node.data.channel_type &&
          (channel.stream ?? 0) === stream,
      );
      if (at >= 0) {
        const [channel] = free.splice(at, 1);
        if (channel !== undefined) {
          bound.set(node.id, channel);
        }
      }
    }
  }
  return bound;
}

export function unboundChannels(
  graph: PatchGraph,
  deviceNode: string,
  set: DeviceSet,
  bound: ReadonlyMap<string, ChannelInfo>,
): ChannelInfo[] {
  const shown = new Set(
    channelNodesOf(graph, deviceNode)
      .map(({ node }) => bound.get(node.id)?.id)
      .filter((id) => id !== undefined),
  );
  return set.channels.filter((channel) => !shown.has(channel.id));
}

export function iqSourceOf(
  graph: PatchGraph,
  node: string,
): { source: string; stream: number } | null {
  for (const edge of graph.edges ?? []) {
    if (edge.to.node !== node || edge.to.port !== "iq") {
      continue;
    }
    const stream = portStream("iq", edge.from.port);
    if (stream !== null) {
      return { source: edge.from.node, stream };
    }
  }
  return null;
}

export function basebandSourceOf(
  graph: PatchGraph,
  node: string,
  devices: ReadonlyMap<string, DeviceSet>,
  channels: ReadonlyMap<string, ChannelInfo>,
): { node: string; deviceSet: number; channel: ChannelInfo } | null {
  for (const source of sourcesOf(graph, node, "baseband")) {
    const channel = channels.get(source);
    const owner = iqSourceOf(graph, source)?.source;
    const set = owner === undefined ? undefined : devices.get(owner);
    if (channel !== undefined && set !== undefined) {
      return { node: source, deviceSet: set.id, channel };
    }
  }
  return null;
}

export function hasWire(graph: PatchGraph, node: string, port: string): boolean {
  return sourcesOf(graph, node, port).length > 0;
}

export function channelNodesOf(
  graph: PatchGraph,
  deviceNode: string,
): { node: PatchNodeOf<"channel">; stream: number }[] {
  const wired: { node: PatchNodeOf<"channel">; stream: number }[] = [];
  for (const node of graph.nodes) {
    if (node.kind !== "channel") {
      continue;
    }
    const input = iqSourceOf(graph, node.id);
    if (input !== null && input.source === deviceNode) {
      wired.push({ node, stream: input.stream });
    }
  }
  return wired;
}

export function deviceNodeOf(graph: PatchGraph, node: string): string | null {
  if (graph.nodes.find((candidate) => candidate.id === node)?.kind === "device") {
    return node;
  }
  const devices = new Set(
    graph.nodes.filter((candidate) => candidate.kind === "device").map((candidate) => candidate.id),
  );
  const upstream = iqSourceOf(graph, node);
  if (upstream !== null && devices.has(upstream.source)) {
    return upstream.source;
  }
  const driven = (graph.edges ?? []).find(
    (edge) => edge.from.node === node && edge.from.port === "control" && devices.has(edge.to.node),
  );
  return driven?.to.node ?? null;
}

export function sourcesOf(graph: PatchGraph, node: string, port: string): string[] {
  return (graph.edges ?? [])
    .filter((edge) => edge.to.node === node && edge.to.port === port)
    .map((edge) => edge.from.node);
}

const MAX_FILTER_DEPTH = 16;

export interface EventPath {
  source: string;
  filters: EventFilterNode[];
}

export function eventPathsOf(graph: PatchGraph, node: string, depth = 0): EventPath[] {
  if (depth > MAX_FILTER_DEPTH) {
    return [];
  }
  return sourcesOf(graph, node, "events").flatMap((source) => {
    const found = (graph.nodes ?? []).find((candidate) => candidate.id === source);
    if (found?.kind !== "event_filter") {
      return [{ source, filters: [] }];
    }
    const settings = found.data ?? {};
    return eventPathsOf(graph, source, depth + 1).map((path) => ({
      source: path.source,
      filters: [...path.filters, settings],
    }));
  });
}

export function eventSourcesOf(graph: PatchGraph, node: string): string[] {
  return [...new Set(eventPathsOf(graph, node).map((path) => path.source))];
}

export function wiredSourcesOf(graph: PatchGraph, node: string): WiredSource[] {
  return eventSourcesOf(graph, node).map((id) => {
    const found = graph.nodes.find((candidate) => candidate.id === id);
    if (found?.kind === "channel") {
      return {
        channelType: found.data.channel_type,
        recordsCalls: found.data.record_calls ?? false,
        trunk: false,
      };
    }
    if (found?.kind === "dmr_trunk") {
      return { recordsCalls: found.data.record_calls ?? true, trunk: true };
    }
    return { recordsCalls: false, trunk: false };
  });
}

export function targetsOf(graph: PatchGraph, node: string, port: string): string[] {
  return (graph.edges ?? [])
    .filter((edge) => edge.from.node === node && edge.from.port === port)
    .map((edge) => edge.to.node);
}

export function inputsOf(
  graph: PatchGraph,
  node: string,
  port: string,
  devices: ReadonlyMap<string, DeviceSet>,
  channels: ReadonlyMap<string, ChannelInfo>,
  trunks: readonly TrunkSystemStatus[] = [],
): Input[] {
  const out: Input[] = [];
  const sources = port === "events" ? eventSourcesOf(graph, node) : sourcesOf(graph, node, port);
  for (const source of sources) {
    const trunk = trunks.find((system) => system.node === source);
    if (trunk !== undefined) {
      out.push(...followerInputs(trunk, devices));
      continue;
    }
    const channel = channels.get(source);
    if (channel === undefined) {
      continue;
    }
    const owner = iqSourceOf(graph, source)?.source;
    const set = owner === undefined ? undefined : devices.get(owner);
    if (set !== undefined) {
      out.push({ node: source, deviceSet: set.id, channel });
    }
  }
  return out;
}

function followerInputs(
  trunk: TrunkSystemStatus,
  devices: ReadonlyMap<string, DeviceSet>,
): Input[] {
  const sets = [...devices.values()];
  return trunk.followers.flatMap((follower) => {
    const channel = sets
      .find((set) => set.id === follower.device_set)
      ?.channels.find((candidate) => candidate.id === follower.channel);
    return channel === undefined
      ? []
      : [{ node: trunk.node, deviceSet: follower.device_set, channel }];
  });
}
