import type {
  ChannelInfo,
  DeviceInfo,
  DeviceRef,
  DeviceSet,
  PatchGraph,
  PatchNodeOf,
  TrunkSystemStatus,
} from "../lib/types";

/** A decoder feeding a sink: which node named it, and the live channel behind it. */
export interface Input {
  node: string;
  deviceSet: number;
  channel: ChannelInfo;
}

import { portStream } from "./graph";

/** The reference that names this discovered device (mirrors `DeviceRef::from_info`). */
export function deviceRefOf(info: DeviceInfo): DeviceRef {
  return {
    backend: info.driver,
    ...(info.serial == null ? {} : { serial: info.serial }),
    ...(info.serial == null || info.key.startsWith(`${info.serial}@`) ? { key: info.key } : {}),
  };
}

/** Whether `info` is the device this reference names (mirrors `DeviceRef::matches`): a serial
 * plus an optional variant key, else the key, else a backend with one serial-less device. */
/** The durable reference that names the device a `driver:key` handle addresses.
 *
 * Split on the *first* colon only, exactly as `DeviceRegistry::open` does: a playback device's
 * key is `file:<path>` and contains colons of its own, and splitting on all of them yields a ref
 * that matches nothing. The key is always carried — a `{backend: "virtual"}` with no key matches
 * any serial-less virtual device, which is to say the signal generator. */
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

/** The radios the other device nodes have already named.
 *
 * A device set binds to at most one node ([`bindDevices`]), and the engine opens a radio once, so
 * naming one of these here would leave a node that can never bind and a radio whose face is
 * somewhere else on the canvas. The picker drops them instead of offering a dead choice. */
export function claimedDevices(graph: PatchGraph, exceptNode: string): DeviceRef[] {
  const claimed: DeviceRef[] = [];
  for (const node of graph.nodes) {
    if (node.kind === "device" && node.id !== exceptNode && node.data.device != null) {
      claimed.push(node.data.device);
    }
  }
  return claimed;
}

/** Device node id → the running device set it drives. A set is claimed by at most one node, in
 * stored node order, so serial-less clones each bind their own. */
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

/**
 * Channel node id → the engine channel it drives, matched by type in stored node order within
 * each device set — the rule the server's apply creates channels by.
 *
 * A channel with no node (added over MCP or by another client) simply has no face; the canvas
 * says so rather than inventing one, and `unboundChannels` is what the device face lists.
 */
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

/**
 * Engine channels of one set that no node on the canvas is showing — a channel added over MCP
 * or by another client, which the device face lists rather than pretending is not there.
 *
 * Scoped to the nodes wired to *this* set on purpose: channel ids are allocated per device set,
 * so filtering the whole binding map by id would let set A's channel 1 mask set B's.
 */
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

/**
 * The IQ wire into `node`: which node feeds it and which receive stream the device end of the
 * wire names — `iq` is stream 0, `iq3` is stream 2 (the `port_stream` resolution in
 * `PatchGraph::channels_of`). `null` while nothing feeds it. Every place that follows an IQ wire
 * resolves it here, so none can read the device end as the bare `iq` and land on stream 0.
 */
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

/**
 * The channel whose baseband tap feeds `node`, and the device set that channel belongs to.
 *
 * `null` while nothing is wired, or while the channel that is wired has no engine channel behind
 * it yet — a node drawn before the server has created it. The two are distinguished by the
 * caller, which has a different thing to say about each.
 */
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

/** Whether anything at all is wired into `node`'s baseband input, bound or not — what tells a
 * face to explain a dead wire rather than to ask for one. */
export function hasBasebandWire(graph: PatchGraph, node: string): boolean {
  return sourcesOf(graph, node, "baseband").length > 0;
}

/** Channel nodes taking IQ from a device node, in stored order, with the stream each one's wire
 * names (mirrors `PatchGraph::channels_of`). */
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

/**
 * The device node behind a node: itself when it is one, the radio feeding its IQ input when it
 * consumes one, and the radio it drives when it is a scanner — ownership runs the other way down
 * the wire, so the scanner's control *output* is what names its radio.
 *
 * One resolver, because every caller means the same question: which radio is this face about.
 */
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

/** Node ids wired into `node`'s named input, in stored order. */
export function sourcesOf(graph: PatchGraph, node: string, port: string): string[] {
  return (graph.edges ?? [])
    .filter((edge) => edge.to.node === node && edge.to.port === port)
    .map((edge) => edge.from.node);
}

/** Node ids fed by `node`'s named output, in stored order. */
export function targetsOf(graph: PatchGraph, node: string, port: string): string[] {
  return (graph.edges ?? [])
    .filter((edge) => edge.from.node === node && edge.from.port === port)
    .map((edge) => edge.to.node);
}

/** The channel faces feeding a sink (speaker, map, log, export), with the device set each one
 * belongs to — everything a sink needs to subscribe to the right streams.
 *
 * A trunk system is expanded to the traffic channels it is following. Those have no node of
 * their own — the follower creates and destroys them as grants come and go — so they are named
 * by the system that owns them, which is what the server stores on their log rows too. */
export function inputsOf(
  graph: PatchGraph,
  node: string,
  port: string,
  devices: ReadonlyMap<string, DeviceSet>,
  channels: ReadonlyMap<string, ChannelInfo>,
  trunks: readonly TrunkSystemStatus[] = [],
): Input[] {
  const out: Input[] = [];
  for (const source of sourcesOf(graph, node, port)) {
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
