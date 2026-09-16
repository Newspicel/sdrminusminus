import type {
  ChannelInfo,
  DeviceInfo,
  DeviceRef,
  DeviceSet,
  EventFilterNode,
  NodeKind,
  PatchGraph,
  PatchNode,
  PatchNodeOf,
  TrunkSystemStatus,
} from "../lib/types";

export interface Input {
  node: string;
  deviceSet: number;
  channel: ChannelInfo;
}

import { portStream } from "./graph";
import { arrayKey } from "./nodes/arrayNode";
import type { WiredSource } from "./nodes/eventFilter";
import { siggenKey } from "./nodes/signalGen";

const SOURCE_KINDS: readonly NodeKind[] = ["device", "recording", "signal_gen", "array"];

export function opensDevice(kind: NodeKind): boolean {
  return SOURCE_KINDS.includes(kind);
}

export function deviceRefOf(info: DeviceInfo): DeviceRef {
  return {
    backend: info.driver,
    ...(info.serial == null ? {} : { serial: info.serial }),
    ...(info.serial == null || info.key.startsWith(`${info.serial}@`) ? { key: info.key } : {}),
  };
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

export function nodeDeviceRef(node: PatchNode): DeviceRef | null {
  switch (node.kind) {
    case "device":
      return node.data.device ?? null;
    case "recording":
      return node.data.recording == null || node.data.recording === ""
        ? null
        : { backend: "recording", key: node.data.recording };
    case "signal_gen":
      return node.data.running ? { backend: "siggen", key: siggenKey(node.id) } : null;
    case "array":
      return { backend: "array", key: arrayKey(node.id) };
    default:
      return null;
  }
}

export function bindDevices(graph: PatchGraph, sets: readonly DeviceSet[]): Map<string, DeviceSet> {
  const bound = new Map<string, DeviceSet>();
  const claimed = new Set<number>();
  for (const node of graph.nodes) {
    const reference = nodeDeviceRef(node);
    if (reference == null) {
      continue;
    }
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

function carries(channel: ChannelInfo, channelType: string, stream: number): boolean {
  return channel.settings.params.type === channelType && (channel.stream ?? 0) === stream;
}

function claim(
  bound: Map<string, ChannelInfo>,
  free: ChannelInfo[],
  node: string,
  at: number,
): void {
  const [channel] = free.splice(at, 1);
  if (channel !== undefined) {
    bound.set(node, channel);
  }
}

export function bindChannels(
  graph: PatchGraph,
  devices: ReadonlyMap<string, DeviceSet>,
): Map<string, ChannelInfo> {
  const bound = new Map<string, ChannelInfo>();
  for (const [deviceNode, set] of devices) {
    const free = [...set.channels];
    const wired = channelNodesOf(graph, deviceNode);
    for (const { node, stream } of wired) {
      const own = free.findIndex(
        (channel) => channel.node === node.id && carries(channel, node.data.channel_type, stream),
      );
      if (own >= 0) {
        claim(bound, free, node.id, own);
      }
    }
    for (const { node, stream } of wired) {
      if (bound.has(node.id)) {
        continue;
      }
      const unclaimed = free.findIndex(
        (channel) => channel.node == null && carries(channel, node.data.channel_type, stream),
      );
      if (unclaimed >= 0) {
        claim(bound, free, node.id, unclaimed);
      }
    }
  }
  return bound;
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
  const kind = graph.nodes.find((candidate) => candidate.id === node)?.kind;
  if (kind !== undefined && opensDevice(kind) && kind !== "array") {
    return node;
  }
  const devices = new Set(
    graph.nodes
      .filter((candidate) => opensDevice(candidate.kind) && candidate.kind !== "array")
      .map((candidate) => candidate.id),
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
      out.push(...trunkInputs(trunk, devices));
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

function trunkInputs(trunk: TrunkSystemStatus, devices: ReadonlyMap<string, DeviceSet>): Input[] {
  const sets = [...devices.values()];
  const wired = (deviceSet: number, channel: number): Input[] => {
    const info = sets
      .find((set) => set.id === deviceSet)
      ?.channels.find((candidate) => candidate.id === channel);
    return info === undefined ? [] : [{ node: trunk.node, deviceSet, channel: info }];
  };
  const control =
    trunk.control == null ? [] : wired(trunk.control.device_set, trunk.control.channel);
  return [
    ...control,
    ...trunk.followers.flatMap((follower) => wired(follower.device_set, follower.channel)),
  ];
}
