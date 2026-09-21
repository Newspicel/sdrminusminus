import type {
  ChannelInfo,
  DeviceInfo,
  DeviceRef,
  DeviceSet,
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

export interface Carrier {
  owner: string;
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

function primaryLane(graph: PatchGraph, node: string, owner: string, stream: number): boolean {
  const first = iqLanesOf(graph, node)[0];
  return first !== undefined && first.source === owner && first.stream === stream;
}

export function bindCarriers(
  graph: PatchGraph,
  devices: ReadonlyMap<string, DeviceSet>,
): Map<string, Carrier> {
  const carriers = new Map<string, Carrier>();
  const claimed = new Map<string, Set<number>>();
  for (const [owner, set] of devices) {
    const used = new Set<number>();
    claimed.set(owner, used);
    for (const { node, stream } of channelNodesOf(graph, owner)) {
      const channel = set.channels.find(
        (live) => live.node === node.id && carries(live, node.data.channel_type, stream),
      );
      if (channel !== undefined && !carriers.has(node.id)) {
        carriers.set(node.id, { owner, channel });
        used.add(channel.id);
      }
    }
  }
  for (const [owner, set] of devices) {
    const used = claimed.get(owner);
    for (const { node, stream } of channelNodesOf(graph, owner)) {
      if (carriers.has(node.id) || !primaryLane(graph, node.id, owner, stream)) {
        continue;
      }
      const channel = set.channels.find(
        (live) =>
          live.node == null && !used?.has(live.id) && carries(live, node.data.channel_type, stream),
      );
      if (channel !== undefined) {
        carriers.set(node.id, { owner, channel });
        used?.add(channel.id);
      }
    }
  }
  return carriers;
}

export function channelsOf(carriers: ReadonlyMap<string, Carrier>): Map<string, ChannelInfo> {
  return new Map([...carriers].map(([node, carrier]) => [node, carrier.channel]));
}

export function ownersOf(carriers: ReadonlyMap<string, Carrier>): Map<string, string> {
  return new Map([...carriers].map(([node, carrier]) => [node, carrier.owner]));
}

export function bindChannels(
  graph: PatchGraph,
  devices: ReadonlyMap<string, DeviceSet>,
): Map<string, ChannelInfo> {
  return channelsOf(bindCarriers(graph, devices));
}

export function iqLanesOf(graph: PatchGraph, node: string): { source: string; stream: number }[] {
  const lanes: { source: string; stream: number }[] = [];
  for (const edge of graph.edges ?? []) {
    if (edge.to.node !== node || edge.to.port !== "iq") {
      continue;
    }
    const stream = portStream("iq", edge.from.port);
    if (stream !== null) {
      lanes.push({ source: edge.from.node, stream });
    }
  }
  return lanes;
}

export function iqSourceOf(
  graph: PatchGraph,
  node: string,
): { source: string; stream: number } | null {
  return iqLanesOf(graph, node)[0] ?? null;
}

const NO_OWNERS: ReadonlyMap<string, string> = new Map();

function carrierOf(
  graph: PatchGraph,
  channel: string,
  owners: ReadonlyMap<string, string>,
): string | undefined {
  return owners.get(channel) ?? iqSourceOf(graph, channel)?.source;
}

export function basebandSourceOf(
  graph: PatchGraph,
  node: string,
  devices: ReadonlyMap<string, DeviceSet>,
  channels: ReadonlyMap<string, ChannelInfo>,
  owners: ReadonlyMap<string, string> = NO_OWNERS,
): { node: string; deviceSet: number; channel: ChannelInfo } | null {
  for (const source of sourcesOf(graph, node, "baseband")) {
    const channel = channels.get(source);
    const owner = carrierOf(graph, source, owners);
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
    for (const lane of iqLanesOf(graph, node.id)) {
      if (lane.source === deviceNode) {
        wired.push({ node, stream: lane.stream });
      }
    }
  }
  return wired;
}

export function deviceNodeOf(
  graph: PatchGraph,
  node: string,
  owners: ReadonlyMap<string, string> = NO_OWNERS,
): string | null {
  const kind = graph.nodes.find((candidate) => candidate.id === node)?.kind;
  if (kind !== undefined && opensDevice(kind)) {
    return node;
  }
  const devices = new Set(
    graph.nodes.filter((candidate) => opensDevice(candidate.kind)).map((candidate) => candidate.id),
  );
  const carrying = (channel: string): string | null => {
    const owner = owners.get(channel);
    if (owner !== undefined && devices.has(owner)) {
      return owner;
    }
    const upstream = iqSourceOf(graph, channel);
    return upstream !== null && devices.has(upstream.source) ? upstream.source : null;
  };
  const own = carrying(node);
  if (own !== null) {
    return own;
  }
  const driven = controlledNodeOf(graph, node);
  return driven === null ? null : carrying(driven);
}

export function controlledNodeOf(graph: PatchGraph, node: string): string | null {
  const driven = (graph.edges ?? []).find(
    (edge) => edge.from.node === node && edge.from.port === "control",
  );
  if (driven === undefined) {
    return null;
  }
  const target = graph.nodes.find((candidate) => candidate.id === driven.to.node);
  return target?.kind === "channel" ? target.id : null;
}

export function sourcesOf(graph: PatchGraph, node: string, port: string): string[] {
  return (graph.edges ?? [])
    .filter((edge) => edge.to.node === node && edge.to.port === port)
    .map((edge) => edge.from.node);
}

const MAX_FILTER_DEPTH = 16;

function walkEventSources(graph: PatchGraph, node: string, depth: number, seen: Set<string>) {
  if (depth > MAX_FILTER_DEPTH) {
    return;
  }
  for (const source of sourcesOf(graph, node, "events")) {
    const found = (graph.nodes ?? []).find((candidate) => candidate.id === source);
    if (found?.kind === "event_filter") {
      walkEventSources(graph, source, depth + 1, seen);
    } else {
      seen.add(source);
    }
  }
}

export function eventSourcesOf(graph: PatchGraph, node: string): string[] {
  const seen = new Set<string>();
  walkEventSources(graph, node, 0, seen);
  return [...seen];
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
  owners: ReadonlyMap<string, string> = NO_OWNERS,
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
    const owner = carrierOf(graph, source, owners);
    const set = owner === undefined ? undefined : devices.get(owner);
    if (set !== undefined) {
      out.push({ node: source, deviceSet: set.id, channel });
    }
  }
  return out;
}

export function speakerInputsOf(
  graph: PatchGraph,
  devices: ReadonlyMap<string, DeviceSet>,
  channels: ReadonlyMap<string, ChannelInfo>,
  trunks: readonly TrunkSystemStatus[] = [],
  owners: ReadonlyMap<string, string> = NO_OWNERS,
): { deviceSet: number; channel: number }[] {
  return graph.nodes
    .filter((node) => node.kind === "speaker")
    .flatMap((node) => inputsOf(graph, node.id, "audio", devices, channels, trunks, owners))
    .map((input) => ({ deviceSet: input.deviceSet, channel: input.channel.id }));
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
