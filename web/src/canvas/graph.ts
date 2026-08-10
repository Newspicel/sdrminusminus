// Pure operations on the stored patch graph (CANVAS §1, §4). No React, no React Flow: the
// canvas maps this model onto the library's nodes and edges, never the other way round, so a
// library major cannot reach the stored station.
//
// The connection rules are enforced twice on purpose — here at drag time, so the operator is
// told where they are looking, and again by the server, which is the one that decides. They are
// not written twice: the port table comes from `GET /api/patch/catalog` and the channel
// specifics from `GET /api/channeltypes`, both generated from `crates/wire/src/patch.rs`.

import { rateMismatch } from "../components/channelSettings";
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
 * The rule itself is the server's (PLAN §18), read off the descriptor rather than re-derived:
 * a decoder that reads the radio's own samples names the range it runs over, and a mode that
 * fills its whole channel names the one rate a resampling DDC could deliver.
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
  const rate = context.bound?.get(from.node)?.settings.sample_rate;
  const wanted = descriptor === undefined ? null : rateMismatch(descriptor, rate);
  if (wanted === null) {
    return null;
  }
  // Short enough to sit on a wire without crossing the patch: the face at the end of it has the
  // room to say why, and does.
  return wanted.min === wanted.max
    ? `needs ${mhz(wanted.min)} MHz`
    : `needs ${mhz(wanted.min)}–${mhz(wanted.max)} MHz`;
}

/** Rates read as MHz in this UI, and a refusal that says 2000000 makes the reader do the sum. */
function mhz(hz: number): string {
  return (hz / 1e6).toFixed(3);
}

// ── how big a face is ─────────────────────────────────────────────────────────────────────

/**
 * The size a face opens at, per kind. Width is always given; **height only for the kinds whose
 * content is a viewport** (a plot, a map, a table) — everything else is left to measure itself,
 * so a node is exactly as tall as what it draws and nothing inside it scrolls (CANVAS §1: the
 * face is the whole control surface, and a control you have to scroll to find is hidden).
 *
 * A stored `size` always wins: it only exists once the operator has resized the node by hand.
 */
export const NODE_SIZE: Record<NodeKind, { w: number; h?: number }> = {
  device: { w: 360 },
  channel: { w: 380 },
  scope: { w: 520, h: 340 },
  speaker: { w: 300 },
  map: { w: 520, h: 380 },
  decoder_log: { w: 760, h: 380 },
  recorder: { w: 300 },
  export: { w: 300 },
  scanner: { w: 400 },
};

/** How far the resizer may shrink a face before its instrument stops being readable. */
export const NODE_MIN_SIZE: Record<NodeKind, { w: number; h: number }> = {
  device: { w: 260, h: 120 },
  channel: { w: 280, h: 120 },
  scope: { w: 320, h: 200 },
  speaker: { w: 220, h: 100 },
  map: { w: 300, h: 220 },
  decoder_log: { w: 360, h: 200 },
  recorder: { w: 220, h: 100 },
  export: { w: 220, h: 100 },
  scanner: { w: 300, h: 160 },
};

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

/**
 * The rack grid (CANVAS §5). Twelve by eight, not the twenty-four squared it shipped as: cells
 * are the unit of every gesture, and a cell an operator cannot aim at is a drag that lands one
 * short. §5 already named the remedy for a rack that feels cramped — bigger cells — and this is
 * it. A face pinned at the default takes a quarter of the rack, so four tile it and nothing has
 * to be resized before it can be read.
 */
export const RACK_COLS = 12;
export const RACK_ROWS = 8;
export const RACK_DEFAULT = { w: 6, h: 4 } as const;

/** Cells a face occupies. The stored shape without the node it belongs to. */
export interface RackCell {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Which side of a face a drag has hold of. */
export type RackEdge = "n" | "e" | "s" | "w";

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

/** Move or resize a pinned face outright. A placement that would overlap or leave the grid is
 * ignored, so a drag can be tracked live and simply refuses to go where it cannot land. */
export function placeSlot(rack: RackLayout, node: string, cell: RackCell): RackLayout {
  const slots = rack.slots ?? [];
  if (!inside(cell) || !slots.every((slot) => slot.node === node || !overlaps(slot, cell))) {
    return rack;
  }
  return { slots: slots.map((slot) => (slot.node === node ? { node, ...cell } : slot)) };
}

/**
 * Move a face to a cell. Dropping it on exactly one other face **trades their places** — the two
 * exchange cells whole, size included, which is what dragging one instrument onto another means
 * on a bench and is the only re-arrangement that cannot fail: the set of occupied cells does not
 * change, so no third face has to move out of the way first.
 *
 * Anything else that would overlap, or leave the grid, is refused rather than clamped.
 */
export function moveSlot(rack: RackLayout, node: string, to: { x: number; y: number }): RackLayout {
  const slots = rack.slots ?? [];
  const from = slots.find((slot) => slot.node === node);
  if (from === undefined) {
    return rack;
  }
  const cell = { x: to.x, y: to.y, w: from.w, h: from.h };
  if (!inside(cell)) {
    return rack;
  }
  const hit = slots.filter((slot) => slot.node !== node && overlaps(slot, cell));
  if (hit.length === 0) {
    return { slots: slots.map((slot) => (slot.node === node ? { node, ...cell } : slot)) };
  }
  const other = hit.length === 1 ? hit[0] : undefined;
  if (other === undefined) {
    return rack;
  }
  return {
    slots: slots.map((slot) => {
      if (slot.node === node) {
        return { node, x: other.x, y: other.y, w: other.w, h: other.h };
      }
      return slot.node === other.node
        ? { node: other.node, x: from.x, y: from.y, w: from.w, h: from.h }
        : slot;
    }),
  };
}

/**
 * Drag one edge of a face by whole cells. The faces on the other side of that edge give up
 * exactly what this one takes: **the boundary between two faces moves, one growing as the other
 * shrinks**, which is the only way to re-balance a rack that is already full without first
 * making a hole in it.
 *
 * An edge with nothing behind it just resizes this face. A drag that would leave any face
 * smaller than a cell, push one off the grid, or open an overlap is refused whole — a live drag
 * stops at the boundary it cannot pass instead of half-applying.
 */
export function resizeSlot(
  rack: RackLayout,
  node: string,
  edge: RackEdge,
  cells: number,
): RackLayout {
  const slots = rack.slots ?? [];
  const slot = slots.find((candidate) => candidate.node === node);
  if (slot === undefined || cells === 0) {
    return rack;
  }
  const pushed = new Set(
    slots.filter((other) => other.node !== node && abuts(slot, other, edge)).map((o) => o.node),
  );
  const next = slots.map((candidate) => {
    if (candidate.node === node) {
      return { node, ...moveEdge(candidate, edge, cells) };
    }
    return pushed.has(candidate.node)
      ? { node: candidate.node, ...moveEdge(candidate, OPPOSITE[edge], cells) }
      : candidate;
  });
  const legal = next.every(
    (cell, index) =>
      inside(cell) && next.every((other, at) => at === index || !overlaps(cell, other)),
  );
  return legal ? { slots: next } : rack;
}

/**
 * Drop slots whose node is gone, and re-place any that no longer fit the grid.
 *
 * The re-placing is what lets the grid change shape: a rack stored against the old twenty-four
 * square one holds cells this one has no room for, and the server validates the *whole* snapshot
 * on every write — so one stale slot would refuse every later write, including a node drag on the
 * canvas that has nothing to do with the rack.
 */
export function pruneRack(rack: RackLayout, graph: PatchGraph): RackLayout {
  const known = new Set(graph.nodes.map((node) => node.id));
  const kept = (rack.slots ?? []).filter((slot) => known.has(slot.node));
  if (kept.length === (rack.slots ?? []).length && kept.every((slot) => inside(slot))) {
    return rack;
  }
  let placed: RackLayout = { slots: kept.filter((slot) => inside(slot)) };
  for (const slot of kept) {
    if (!inside(slot)) {
      placed = pin(placed, slot.node);
    }
  }
  return placed;
}

function inside(cell: RackCell): boolean {
  return (
    cell.w >= 1 &&
    cell.h >= 1 &&
    cell.x >= 0 &&
    cell.y >= 0 &&
    cell.x + cell.w <= RACK_COLS &&
    cell.y + cell.h <= RACK_ROWS
  );
}

const OPPOSITE: Record<RackEdge, RackEdge> = { n: "s", e: "w", s: "n", w: "e" };

/** The cell with one edge moved by `cells` — positive is rightwards / downwards, so the same
 * delta moves a shared boundary the same way from either side of it. */
function moveEdge(cell: RackCell, edge: RackEdge, cells: number): RackCell {
  switch (edge) {
    case "n":
      return { ...cell, y: cell.y + cells, h: cell.h - cells };
    case "s":
      return { ...cell, h: cell.h + cells };
    case "w":
      return { ...cell, x: cell.x + cells, w: cell.w - cells };
    case "e":
      return { ...cell, w: cell.w + cells };
  }
}

/** Whether `other` sits against `cell`'s named edge, sharing some of it — the faces a drag on
 * that edge takes with it. Touching only at a corner is not sharing a boundary. */
function abuts(cell: RackCell, other: RackCell, edge: RackEdge): boolean {
  const spansX = other.x < cell.x + cell.w && cell.x < other.x + other.w;
  const spansY = other.y < cell.y + cell.h && cell.y < other.y + other.h;
  switch (edge) {
    case "n":
      return spansX && other.y + other.h === cell.y;
    case "s":
      return spansX && other.y === cell.y + cell.h;
    case "w":
      return spansY && other.x + other.w === cell.x;
    case "e":
      return spansY && other.x === cell.x + cell.w;
  }
}

function overlaps(a: RackCell, b: RackCell): boolean {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}
