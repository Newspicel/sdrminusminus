// Which engine object each node is driving right now (CANVAS §3).
//
// Bindings are computed, never stored: engine ids are allocated per run and reused, so a stored
// one would silently attach a node to whichever radio opened first. This mirrors
// `apply_station` in `crates/server/src/rest.rs` and `DeviceRef::matches` in
// `crates/wire/src/patch.rs` — the same two rules, so the face the canvas draws is the channel
// the server's apply would have created. Both sides carry tests; if one changes, both change.

import type { ChannelInfo, DeviceInfo, DeviceRef, DeviceSet, PatchGraph } from "../lib/types";

/** The reference that names this discovered device (mirrors `DeviceRef::from_info`). */
export function deviceRefOf(info: DeviceInfo): DeviceRef {
  return {
    backend: info.driver,
    ...(info.serial == null ? { key: info.key } : { serial: info.serial }),
  };
}

/** Whether `info` is the device this reference names (mirrors `DeviceRef::matches`): serial when
 * the driver exposes one, else the key, else a backend with a single serial-less device. */
export function refMatches(reference: DeviceRef, info: DeviceInfo): boolean {
  if (reference.backend !== info.driver) {
    return false;
  }
  if (reference.serial != null) {
    return reference.serial === info.serial;
  }
  return reference.key == null || reference.key === info.key;
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
    for (const node of channelNodesOf(graph, deviceNode)) {
      if (node.kind !== "channel") {
        continue;
      }
      const at = free.findIndex(
        (channel) => channel.settings.params.type === node.data.channel_type,
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
      .map((node) => bound.get(node.id)?.id)
      .filter((id) => id !== undefined),
  );
  return set.channels.filter((channel) => !shown.has(channel.id));
}

/** Channel nodes taking IQ from a device node, in stored order (mirrors `PatchGraph::channels_of`). */
export function channelNodesOf(graph: PatchGraph, deviceNode: string) {
  const edges = graph.edges ?? [];
  return graph.nodes.filter(
    (node) =>
      node.kind === "channel" &&
      edges.some(
        (edge) =>
          edge.to.node === node.id && edge.to.port === "iq" && edge.from.node === deviceNode,
      ),
  );
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
 * belongs to — everything a sink needs to subscribe to the right streams. */
export function inputsOf(
  graph: PatchGraph,
  node: string,
  port: string,
  devices: ReadonlyMap<string, DeviceSet>,
  channels: ReadonlyMap<string, ChannelInfo>,
): { node: string; deviceSet: number; channel: ChannelInfo }[] {
  const out: { node: string; deviceSet: number; channel: ChannelInfo }[] = [];
  for (const source of sourcesOf(graph, node, port)) {
    const channel = channels.get(source);
    if (channel === undefined) {
      continue;
    }
    const owner = sourcesOf(graph, source, "iq")[0];
    const set = owner === undefined ? undefined : devices.get(owner);
    if (set !== undefined) {
      out.push({ node: source, deviceSet: set.id, channel });
    }
  }
  return out;
}
