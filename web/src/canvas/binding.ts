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
  fx: string[];
}

export interface AudioSource {
  node: string;
  fx: string[];
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

function primaryLane(
  graph: PatchGraph,
  node: string,
  owner: string,
  stream: number,
  devices: ReadonlyMap<string, DeviceSet>,
): boolean {
  const first = iqLanesOf(graph, node, devices)[0];
  return first !== undefined && first.source === owner && first.stream === stream;
}

export function trunkChannelIds(
  trunks: readonly TrunkSystemStatus[],
  deviceSet: number,
): Set<number> {
  const ids = new Set<number>();
  for (const system of trunks) {
    if (system.control != null && system.control.device_set === deviceSet) {
      ids.add(system.control.channel);
    }
    for (const follower of system.followers) {
      if (follower.device_set === deviceSet) {
        ids.add(follower.channel);
      }
    }
    for (const probe of system.probes ?? []) {
      if (probe.device_set === deviceSet) {
        ids.add(probe.channel);
      }
    }
  }
  return ids;
}

export function bindCarriers(
  graph: PatchGraph,
  devices: ReadonlyMap<string, DeviceSet>,
  trunks: readonly TrunkSystemStatus[] = [],
): Map<string, Carrier> {
  const carriers = new Map<string, Carrier>();
  const claimed = new Map<string, Set<number>>();
  for (const [owner, set] of devices) {
    const used = trunkChannelIds(trunks, set.id);
    claimed.set(owner, used);
    for (const { node, stream } of channelNodesOf(graph, owner, devices)) {
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
    for (const { node, stream } of channelNodesOf(graph, owner, devices)) {
      if (carriers.has(node.id) || !primaryLane(graph, node.id, owner, stream, devices)) {
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
  trunks: readonly TrunkSystemStatus[] = [],
): Map<string, ChannelInfo> {
  return channelsOf(bindCarriers(graph, devices, trunks));
}

export interface IqLane {
  source: string;
  stream: number;
  beam?: { node: string; port: string; tunes: number };
}

const LANE_OUTPUTS: Readonly<Partial<Record<NodeKind, string>>> = {
  df: "beam",
  combiner: "beam",
  stitch: "wide",
};
const NO_DEVICES: ReadonlyMap<string, DeviceSet> = new Map();

export function laneOutputOf(kind: NodeKind | undefined): string | undefined {
  return kind === undefined ? undefined : LANE_OUTPUTS[kind];
}

function beamLane(
  graph: PatchGraph,
  node: string,
  port: string,
  devices: ReadonlyMap<string, DeviceSet>,
): IqLane | null {
  const kind = graph.nodes.find((candidate) => candidate.id === node)?.kind;
  if (laneOutputOf(kind) !== port) {
    return null;
  }
  const element = directLanesOf(graph, node)[0];
  if (element === undefined) {
    return null;
  }
  const stream = devices.get(element.source)?.capabilities.rx_streams ?? -1;
  return {
    source: element.source,
    stream,
    beam: { node, port, tunes: kind === "stitch" ? stream : element.stream },
  };
}

function directLanesOf(graph: PatchGraph, node: string): IqLane[] {
  const lanes: IqLane[] = [];
  for (const edge of graph.edges ?? []) {
    if (edge.to.node !== node || portStream("iq", edge.to.port) === null) {
      continue;
    }
    const stream = portStream("iq", edge.from.port);
    if (stream !== null) {
      lanes.push({ source: edge.from.node, stream });
    }
  }
  return lanes.toSorted((a, b) => a.stream - b.stream);
}

export function iqLanesOf(
  graph: PatchGraph,
  node: string,
  devices: ReadonlyMap<string, DeviceSet> = NO_DEVICES,
): IqLane[] {
  const lanes: IqLane[] = [];
  for (const edge of graph.edges ?? []) {
    if (edge.to.node !== node || edge.to.port !== "iq") {
      continue;
    }
    const stream = portStream("iq", edge.from.port);
    if (stream !== null) {
      lanes.push({ source: edge.from.node, stream });
      continue;
    }
    const beam = beamLane(graph, edge.from.node, edge.from.port, devices);
    if (beam !== null) {
      lanes.push(beam);
    }
  }
  return lanes;
}

export function tunedStream(lane: IqLane): number {
  return lane.beam?.tunes ?? lane.stream;
}

export function iqSourceOf(
  graph: PatchGraph,
  node: string,
  devices: ReadonlyMap<string, DeviceSet> = NO_DEVICES,
): IqLane | null {
  return iqLanesOf(graph, node, devices)[0] ?? null;
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
  devices: ReadonlyMap<string, DeviceSet> = NO_DEVICES,
): { node: PatchNodeOf<"channel">; stream: number }[] {
  const wired: { node: PatchNodeOf<"channel">; stream: number }[] = [];
  for (const node of graph.nodes) {
    if (node.kind !== "channel") {
      continue;
    }
    for (const lane of iqLanesOf(graph, node.id, devices)) {
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

const TUNING_CONTROLLERS: Readonly<Partial<Record<PatchNode["kind"], string>>> = {
  scanner: "Scanner",
  satellite: "Satellite",
};

export function tuningControllerOf(graph: PatchGraph, channel: string): string | null {
  const wire = (graph.edges ?? []).find(
    (edge) => edge.to.node === channel && edge.to.port === "control",
  );
  const controller = graph.nodes.find((candidate) => candidate.id === wire?.from.node);
  const kind = controller === undefined ? undefined : TUNING_CONTROLLERS[controller.kind];
  return kind === undefined ? null : (controller?.label ?? kind);
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

function walkAudioSources(
  graph: PatchGraph,
  node: string,
  fx: string[],
  found: AudioSource[],
): void {
  if (fx.length > MAX_FILTER_DEPTH) {
    return;
  }
  for (const source of sourcesOf(graph, node, "audio")) {
    const upstream = graph.nodes.find((candidate) => candidate.id === source);
    if (upstream?.kind === "audio_fx") {
      walkAudioSources(graph, source, [source, ...fx], found);
    } else {
      found.push({ node: source, fx });
    }
  }
}

export function audioSourcesOf(graph: PatchGraph, node: string): AudioSource[] {
  const found: AudioSource[] = [];
  walkAudioSources(graph, node, [], found);
  return found;
}

function sourcesFor(graph: PatchGraph, node: string, port: string): AudioSource[] {
  if (port === "audio") {
    return audioSourcesOf(graph, node);
  }
  const plain = port === "events" ? eventSourcesOf(graph, node) : sourcesOf(graph, node, port);
  return plain.map((source) => ({ node: source, fx: [] }));
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
    if (found?.kind === "spectrum_monitor") {
      return { recordsCalls: false, trunk: false, monitor: true };
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
  for (const { node: source, fx } of sourcesFor(graph, node, port)) {
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
      out.push({ node: source, deviceSet: set.id, channel, fx });
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
): { deviceSet: number; channel: number; fx: string[] }[] {
  return graph.nodes
    .filter((node) => node.kind === "speaker")
    .flatMap((node) => inputsOf(graph, node.id, "audio", devices, channels, trunks, owners))
    .map((input) => ({ deviceSet: input.deviceSet, channel: input.channel.id, fx: input.fx }));
}

function trunkInputs(trunk: TrunkSystemStatus, devices: ReadonlyMap<string, DeviceSet>): Input[] {
  const sets = [...devices.values()];
  const wired = (deviceSet: number, channel: number): Input[] => {
    const info = sets
      .find((set) => set.id === deviceSet)
      ?.channels.find((candidate) => candidate.id === channel);
    return info === undefined ? [] : [{ node: trunk.node, deviceSet, channel: info, fx: [] }];
  };
  const control =
    trunk.control == null ? [] : wired(trunk.control.device_set, trunk.control.channel);
  return [
    ...control,
    ...trunk.followers.flatMap((follower) => wired(follower.device_set, follower.channel)),
  ];
}
