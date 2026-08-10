// Pure operations on the stored patch graph (CANVAS §1, §4). No React, no React Flow: the
// canvas maps this model onto the library's nodes and edges, never the other way round, so a
// library major cannot reach the stored station.
//
// The connection rules are enforced twice on purpose — here at drag time, so the operator is
// told where they are looking, and again by the server, which is the one that decides. They are
// not written twice: the port table comes from `GET /api/patch/catalog` and the channel
// specifics from `GET /api/channeltypes`, both generated from `crates/wire/src/patch.rs`.

import type {
  ChannelDescriptor,
  DeviceSet,
  NodeKind,
  PatchCatalog,
  PatchEdge,
  PatchGraph,
  PatchNode,
  PortRef,
  PortSpec,
  RackLayout,
} from "../lib/types";

/** Everything the rules need that is not the graph itself. */
export interface GraphContext {
  catalog: PatchCatalog;
  channelTypes: readonly ChannelDescriptor[];
  /** Device sets by node id, from `bindDevices` — only needed for the live rate rule. */
  bound?: ReadonlyMap<string, DeviceSet>;
}

export function nodeOf(graph: PatchGraph, id: string): PatchNode | undefined {
  return graph.nodes.find((node) => node.id === id);
}

/** A fresh node id. Ids only have to be unique within one graph and stable for the node's life;
 * `crypto.randomUUID` is available in every browser this ships to and needs no counter that a
 * second client could collide with. */
export function newNodeId(kind: NodeKind): string {
  return `${kind}:${crypto.randomUUID().slice(0, 8)}`;
}

export function descriptorOf(
  context: GraphContext,
  node: PatchNode,
): ChannelDescriptor | undefined {
  return node.kind === "channel"
    ? context.channelTypes.find((type) => type.type_id === node.data.channel_type)
    : undefined;
}

/** The ports this node actually has: the catalog's table for its kind, with a channel's
 * conditional outputs resolved against its type. */
export function portsOf(context: GraphContext, node: PatchNode): PortSpec[] {
  const entry = context.catalog.nodes.find((type) => type.kind === node.kind);
  if (entry === undefined) {
    return [];
  }
  const descriptor = descriptorOf(context, node);
  return entry.ports.filter((port) => {
    switch (port.condition) {
      case "channel_has_audio":
        return descriptor?.has_audio === true;
      case "channel_is_decoder":
        return descriptor?.decoder_kind != null;
      default:
        return true;
    }
  });
}

export function portOf(
  context: GraphContext,
  graph: PatchGraph,
  reference: PortRef,
): PortSpec | undefined {
  const node = nodeOf(graph, reference.node);
  return node === undefined
    ? undefined
    : portsOf(context, node).find((port) => port.name === reference.port);
}

export function edgeKey(edge: PatchEdge): string {
  return `${edge.from.node}.${edge.from.port}->${edge.to.node}.${edge.to.port}`;
}

/**
 * Why this wire cannot be drawn, or `null` if it can. The reason is the message the operator
 * sees on the edge (CANVAS §1: "an invalid wire is refused with the reason where the operator
 * is looking"), so it names the fix wherever there is one.
 */
export function connectionRefusal(
  context: GraphContext,
  graph: PatchGraph,
  from: PortRef,
  to: PortRef,
): string | null {
  if (from.node === to.node) {
    return "a node cannot wire to itself";
  }
  const out = portOf(context, graph, from);
  const input = portOf(context, graph, to);
  if (out === undefined || input === undefined) {
    return "that port does not exist";
  }
  if (out.direction !== "out" || input.direction !== "in") {
    return "wires run from an output to an input";
  }
  if (out.port_type !== input.port_type) {
    return `${out.port_type} cannot feed a ${input.port_type} input`;
  }
  const landing = (graph.edges ?? []).filter(
    (edge) => edge.to.node === to.node && edge.to.port === to.port,
  );
  if (landing.some((edge) => edge.from.node === from.node && edge.from.port === from.port)) {
    return "already wired";
  }
  if (!input.multi && landing.length > 0) {
    return nodeOf(graph, to.node)?.kind === "channel"
      ? "a channel takes one receiver; two would need a coherent array"
      : "that input takes one wire";
  }
  return null;
}

/**
 * What is wrong with a wire that was allowed to exist (CANVAS §1: the rate rule "surfaces on the
 * wire … a visible wire error, not a buried log line"). A rate is one setting away, so it is not
 * a reason to refuse the connection — the operator meant to put ADS-B on that radio, and the
 * answer is to change the rate, not to pretend the two cannot be joined.
 *
 * A mode that fills its whole channel leaves a resampler no guard band, so at any other rate it
 * decodes nothing at all (PLAN §18). The flag is the server's — derived from the same functions
 * the engine's admission check uses — not a number copied into the client.
 */
export function edgeWarning(
  context: GraphContext,
  graph: PatchGraph,
  from: PortRef,
  to: PortRef,
): string | null {
  const channel = nodeOf(graph, to.node);
  if (channel?.kind !== "channel") {
    return null;
  }
  const descriptor = descriptorOf(context, channel);
  if (descriptor?.exact_rate_only !== true) {
    return null;
  }
  const rate = context.bound?.get(from.node)?.settings.sample_rate;
  if (rate == null || rate === descriptor.input_rate_hz) {
    return null;
  }
  // Short enough to sit on a wire without crossing the patch: the face at the end of it has the
  // room to say why, and does.
  return `needs ${mhz(descriptor.input_rate_hz)} MHz`;
}

/** Rates read as MHz in this UI, and a refusal that says 2000000 makes the reader do the sum. */
function mhz(hz: number): string {
  return (hz / 1e6).toFixed(3);
}

/** Add a node. The caller supplies the id so the same call can be replayed optimistically. */
export function addNode(graph: PatchGraph, node: PatchNode): PatchGraph {
  return { ...graph, nodes: [...graph.nodes, node] };
}

/** Remove a node and every wire that touched it. */
export function removeNode(graph: PatchGraph, id: string): PatchGraph {
  return {
    nodes: graph.nodes.filter((node) => node.id !== id),
    edges: (graph.edges ?? []).filter((edge) => edge.from.node !== id && edge.to.node !== id),
  };
}

export function addEdge(graph: PatchGraph, edge: PatchEdge): PatchGraph {
  return { ...graph, edges: [...(graph.edges ?? []), edge] };
}

export function removeEdge(graph: PatchGraph, key: string): PatchGraph {
  return { ...graph, edges: (graph.edges ?? []).filter((edge) => edgeKey(edge) !== key) };
}

/** Replace one node, leaving the rest of the graph identical. */
export function patchNode(
  graph: PatchGraph,
  id: string,
  edit: (node: PatchNode) => PatchNode,
): PatchGraph {
  return {
    ...graph,
    nodes: graph.nodes.map((node) => (node.id === id ? edit(node) : node)),
  };
}

/** Structural equality, used to drop an echo of our own write and an aborted drag. Stringify is
 * enough because every producer builds these objects in field order from the same code. */
export function sameGraph(a: PatchGraph, b: PatchGraph): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

// ── the rack ──────────────────────────────────────────────────────────────────────────────

/** Default cells a newly pinned face occupies. Wide enough for a scope, and six of them tile the
 * grid — two across, three down. */
export const RACK_DEFAULT = { w: 12, h: 8 } as const;
export const RACK_COLS = 24;
export const RACK_ROWS = 24;

export function isPinned(rack: RackLayout, node: string): boolean {
  return (rack.slots ?? []).some((slot) => slot.node === node);
}

/** Pin a face at the first free cell, scanning left-to-right then down. Returns the rack
 * unchanged when it is already pinned or there is nowhere it fits — a full rack is a rack, not
 * an error. */
export function pin(rack: RackLayout, node: string, size = RACK_DEFAULT): RackLayout {
  if (isPinned(rack, node)) {
    return rack;
  }
  const slots = rack.slots ?? [];
  for (let y = 0; y + size.h <= RACK_ROWS; y++) {
    for (let x = 0; x + size.w <= RACK_COLS; x++) {
      const cell = { x, y, w: size.w, h: size.h };
      if (!slots.some((slot) => overlaps(slot, cell))) {
        return { slots: [...slots, { node, ...cell }] };
      }
    }
  }
  return rack;
}

export function unpin(rack: RackLayout, node: string): RackLayout {
  return { slots: (rack.slots ?? []).filter((slot) => slot.node !== node) };
}

/** Move or resize a pinned face. A move that would overlap or leave the grid is ignored, so a
 * drag can be tracked live and simply refuses to go where it cannot land. */
export function placeSlot(
  rack: RackLayout,
  node: string,
  cell: { x: number; y: number; w: number; h: number },
): RackLayout {
  const slots = rack.slots ?? [];
  const inside =
    cell.w >= 1 &&
    cell.h >= 1 &&
    cell.x >= 0 &&
    cell.y >= 0 &&
    cell.x + cell.w <= RACK_COLS &&
    cell.y + cell.h <= RACK_ROWS;
  const clear = slots.every((slot) => slot.node === node || !overlaps(slot, cell));
  if (!inside || !clear) {
    return rack;
  }
  return {
    slots: slots.map((slot) => (slot.node === node ? { node, ...cell } : slot)),
  };
}

/** Drop slots whose node is gone — a rack entry for a deleted node fails validation. */
export function pruneRack(rack: RackLayout, graph: PatchGraph): RackLayout {
  const known = new Set(graph.nodes.map((node) => node.id));
  const slots = (rack.slots ?? []).filter((slot) => known.has(slot.node));
  return slots.length === (rack.slots ?? []).length ? rack : { slots };
}

function overlaps(
  a: { x: number; y: number; w: number; h: number },
  b: { x: number; y: number; w: number; h: number },
): boolean {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}
