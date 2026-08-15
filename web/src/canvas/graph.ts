import { channelHasAudio, rateMismatch } from "../components/channelSettings";
import type {
  Capabilities,
  ChannelDescriptor,
  DeviceSet,
  Duplex,
  NodeKind,
  PatchCatalog,
  PatchEdge,
  PatchGraph,
  PatchNode,
  PortDirection,
  PortRef,
  PortSpec,
  RackLayout,
  WorkspaceSnapshot,
} from "../lib/types";

/** Everything the rules need that is not the graph itself. */
export interface GraphContext {
  catalog: PatchCatalog;
  channelTypes: readonly ChannelDescriptor[];
  /** Device sets by node id, from `bindDevices`. The rules that read what a radio *is* rather
   * than what the patch says — the live rate warning, and whether a node has a transmit input —
   * are answered from here; absent means the radio is not attached. */
  bound?: ReadonlyMap<string, DeviceSet>;
}

export function nodeOf(graph: PatchGraph, id: string): PatchNode | undefined {
  return graph.nodes.find((node) => node.id === id);
}

//
// A multi-stream radio has one IQ output per receive stream. The three rules below are
// `crates/wire/src/patch.rs`'s `MAX_STREAMS` / `stream_port` / `port_stream`, mirrored so the
// canvas names the same ports the server validates; if one side changes, both change.

export const MAX_STREAMS = 16;

/** The port name for stream `index` of the family `base`. Stream 0 keeps the bare name — every
 * stored workspace and template names it — so the wire's numbering starts at 2: `iq`, `iq2`… */
export function streamPort(base: string, index: number): string {
  return index === 0 ? base : `${base}${index + 1}`;
}

/** What a port is *called on screen*, which is not always what it is called on the wire.
 *
 * Stream 0 is stored as the bare `iq`, and renaming it would invalidate every stored workspace and
 * template — but on screen an unnumbered `IQ` sitting above `IQ2` reads as a different kind of
 * port rather than the first of a set. So it is shown as `IQ1` whenever the radio has a second
 * stream to tell it from, and left bare when it is the only one.
 *
 * `siblings` is the port list it is drawn among, already expanded per stream. */
export function portLabel(name: string, siblings: readonly PortSpec[]): string {
  const numbered = `${name}2`;
  return siblings.some((port) => port.name === numbered) ? `${name}1` : name;
}

/** [`portLabel`] for a control that is drawn per stream rather than per port: the face knows how
 * many lanes it is rendering, so it has no port list to look a sibling up in. */
export function streamLabel(base: string, index: number, streams: number): string {
  return streams > 1 && index === 0 ? `${base}1` : streamPort(base, index);
}

/** How many receive streams to draw controls for: the declared count, clamped the same way the
 * port family is, so a dial or gain row always has the socket it reads as. */
export function rxStreamCount(capabilities: Capabilities | undefined): number {
  return clampStreams(capabilities?.rx_streams);
}

/** No radio attached (or an older server that reports no count): stream 0 only. */
function clampStreams(declared: number | undefined): number {
  return Math.min(Math.max(declared ?? 1, 1), MAX_STREAMS);
}

/** The stream `name` addresses within family `base`, or `null` when it is not one of that
 * family's. One spelling per port: `iq1` would alias `iq` and `iq02` would alias `iq2`, so only
 * the canonical rendering of 2..=MAX_STREAMS names a stream. */
export function portStream(base: string, name: string): number | null {
  if (name === base) {
    return 0;
  }
  if (!name.startsWith(base)) {
    return null;
  }
  const suffix = name.slice(base.length);
  const n = Number(suffix);
  if (!Number.isInteger(n) || n < 2 || n > MAX_STREAMS || suffix !== String(n)) {
    return null;
  }
  return n - 1;
}

/** `crates/wire/src/patch.rs`'s `MAX_NODES` / `MAX_EDGES`. The server validates the whole snapshot
 * on every write, so a gesture that would cross either is refused where it is made rather than
 * taking the next write down with it. */
export const MAX_NODES = 128;
export const MAX_EDGES = 256;

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

/**
 * The ports this node actually has: the catalog's table for its kind, with the conditional ones
 * resolved against what is behind the node — a channel's type, a device's radio (mirrors
 * `PortSpec::applies_to`).
 *
 * A device's transmit input is the one port that depends on live state rather than on the stored
 * patch: which radio a node names is stored, what that radio can do is only known while it is
 * attached. So an unbound node has no transmit input, and gains one when its transceiver appears
 * — the same way its dial only reads a frequency once there is a radio behind it.
 */
/** Whether a radio has a send side at all. `rx_only` is the wire default, so a capability set
 * that says nothing — an older server, or a backend that declares no duplex — has none. */
export function hasTransmitter(duplex: Duplex | undefined): boolean {
  return duplex === "half" || duplex === "full" || duplex === "tx_only";
}

export function portsOf(context: GraphContext, graph: PatchGraph, node: PatchNode): PortSpec[] {
  const entry = context.catalog.nodes.find((type) => type.kind === node.kind);
  if (entry === undefined) {
    return [];
  }
  const descriptor = descriptorOf(context, node);
  const capabilities = context.bound?.get(node.id)?.capabilities;
  return entry.ports
    .filter((port) => {
      switch (port.condition ?? "always") {
        case "always":
          return true;
        case "channel_has_audio":
          return descriptor?.has_audio === true;
        case "channel_is_decoder":
          return descriptor?.decoder_kind != null;
        case "channel_has_video":
          return descriptor?.has_video === true;
        case "channel_needs_position":
          return descriptor?.needs_position === true;
        case "device_is_tx_capable":
          // The reserved transmit input is drawn on a radio that *has* a send side, whatever
          //  lets it do with one: `rx_only` is the wire default, so a radio that says
          // nothing gets no port.
          return hasTransmitter(capabilities?.duplex);
        default:
          // A condition this build has no answer for, exactly as `PortSpec::applies_to` reads it:
          // a port drawn without checking is one the operator can be told to wire and then
          // refused, which is how the video output arrived on every channel.
          return false;
      }
    })
    .flatMap((port) => expandStreams(port, node.id, graph.edges ?? [], capabilities));
}

/**
 * One concrete port per stream for a repeating spec (mirrors `NodeBody::ports_with`): the
 * catalog is per-build static, so how many streams a family really has is read off the attached
 * radio's capabilities.
 *
 * Streams a stored wire already names are always kept, *unlike* the transmit input's
 * hide-when-unbound rule: an edge whose port has no handle is an edge React Flow will not draw,
 * so a workspace laid out against a four-stream radio must not lose its wires the moment that
 * radio is absent — or smaller than it was.
 */
function expandStreams(
  spec: PortSpec,
  node: string,
  edges: readonly PatchEdge[],
  capabilities: Capabilities | undefined,
): PortSpec[] {
  const repeat = spec.repeat ?? "once";
  if (repeat === "once") {
    return [spec];
  }
  const count = clampStreams(
    repeat === "per_rx_stream" ? capabilities?.rx_streams : capabilities?.tx_streams,
  );
  const streams = new Set<number>();
  for (let stream = 0; stream < count; stream++) {
    streams.add(stream);
  }
  for (const edge of edges) {
    const end = spec.direction === "out" ? edge.from : edge.to;
    const stream = end.node === node ? portStream(spec.name, end.port) : null;
    if (stream !== null) {
      streams.add(stream);
    }
  }
  // Expanded ports are concrete sockets, so they carry `once` — leaving the flag on would
  // invite a second expansion.
  return [...streams]
    .toSorted((a, b) => a - b)
    .map((stream) => ({ ...spec, name: streamPort(spec.name, stream), repeat: "once" as const }));
}

export function portOf(
  context: GraphContext,
  graph: PatchGraph,
  reference: PortRef,
  direction?: PortDirection,
): PortSpec | undefined {
  const node = nodeOf(graph, reference.node);
  return node === undefined
    ? undefined
    : portsOf(context, graph, node).find(
        (port) =>
          port.name === reference.port && (direction === undefined || port.direction === direction),
      );
}

export function edgeKey(edge: PatchEdge): string {
  return `${edge.from.node}.${edge.from.port}->${edge.to.node}.${edge.to.port}`;
}

export function connectionRefusal(
  context: GraphContext,
  graph: PatchGraph,
  from: PortRef,
  to: PortRef,
): string | null {
  if (from.node === to.node) {
    return "a node cannot wire to itself";
  }
  const out = portOf(context, graph, from, "out");
  const input = portOf(context, graph, to, "in");
  if (out === undefined || input === undefined) {
    if (
      portOf(context, graph, from, "in") !== undefined ||
      portOf(context, graph, to, "out") !== undefined
    ) {
      return "wires run from an output to an input";
    }
    return "that port does not exist";
  }
  if (out.direction !== "out" || input.direction !== "in") {
    return "wires run from an output to an input";
  }
  if (out.port_type !== input.port_type) {
    return input.note ?? `${out.port_type} cannot feed a ${input.port_type} input`;
  }
  const edges = graph.edges ?? [];
  const landing = edges.filter((edge) => edge.to.node === to.node && edge.to.port === to.port);
  if (landing.some((edge) => edge.from.node === from.node && edge.from.port === from.port)) {
    return "already wired";
  }
  if (!input.multi && landing.length > 0) {
    return nodeOf(graph, to.node)?.kind === "channel"
      ? "a channel takes one device; two would need a coherent array"
      : "that input takes one wire";
  }
  // A stream output fans out; an ownership output does not, and the server refuses the second
  // wire either way — one sweep is what the engine runs.
  if (
    !out.multi &&
    edges.some((edge) => edge.from.node === from.node && edge.from.port === from.port)
  ) {
    return "that output drives one node at a time";
  }
  return null;
}

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

/**
 * The size a face is. Every kind states both axes and most kinds are only ever that size
 * (`isResizable`), so a control is laid out once against a width that is known here rather than
 * against whatever the operator last dragged — no breakpoints, no reflow, and two radios on one
 * canvas are the same box whether or not their drivers declare the same settings.
 */
export const NODE_SIZE: Record<NodeKind, { w: number; h: number }> = {
  device: { w: 380, h: 420 },
  gps: { w: 360, h: 260 },
  channel: { w: 440, h: 300 },
  chat_output: { w: 420, h: 330 },
  scope: { w: 520, h: 360 },
  speaker: { w: 320, h: 200 },
  map: { w: 520, h: 380 },
  signal_map: { w: 600, h: 440 },
  readout: { w: 560, h: 320 },
  decoder_log: { w: 720, h: 380 },
  dmr_trunk: { w: 480, h: 360 },
  video: { w: 380, h: 320 },
  recorder: { w: 340, h: 260 },
  network_export: { w: 380, h: 310 },
  export: { w: 320, h: 180 },
  scanner: { w: 400, h: 400 },
};

/**
 * What a channel face opens at once it carries the audio-processing chain.
 *
 * A channel is the one kind with two right sizes. Every mode that produces audio has the whole
 * chain in its column — AGC, blanker, denoise, auto-notch, passband — and every mode that only
 * decodes has none of it, so one height either scrolls the voice modes or leaves an acre of dead
 * space under the data ones. Tall enough for the deepest voice column plus the row NFM's tone
 * squelch adds and a notch beside it; past that the face scrolls, which is the right answer for
 * a channel carrying four notches at once.
 */
const AUDIO_CHANNEL_H = 500;

/** The size `node`'s face opens at. */
export function naturalSize(node: PatchNode, context: GraphContext): { w: number; h: number } {
  return node.kind === "channel"
    ? channelSize(channelHasAudio(descriptorOf(context, node)))
    : NODE_SIZE[node.kind];
}

/** [`naturalSize`] for a channel that does not exist yet, which is what a drop is placing. */
export function channelSize(hasAudio: boolean): { w: number; h: number } {
  return { w: NODE_SIZE.channel.w, h: hasAudio ? AUDIO_CHANNEL_H : NODE_SIZE.channel.h };
}

/**
 * Whether this kind can be resized at all.
 *
 * Only the faces whose content is a viewport — a plot, a map, a table, a picture — where more
 * room is more of the instrument. A face that is a column of controls has one right size, and
 * leaving it draggable meant every patch drifted into a set of boxes at fourteen different
 * heights for no reading gained.
 */
export function isResizable(kind: NodeKind): boolean {
  return RESIZABLE.has(kind);
}

const RESIZABLE = new Set<NodeKind>([
  "scope",
  "map",
  "signal_map",
  "readout",
  "decoder_log",
  "dmr_trunk",
  "video",
]);

/** How far the resizer may shrink a face before its instrument stops being readable. Only the
 * resizable kinds have one; the rest are the size they are. */
export const NODE_MIN_SIZE: Record<NodeKind, { w: number; h: number }> = {
  device: NODE_SIZE.device,
  gps: NODE_SIZE.gps,
  channel: NODE_SIZE.channel,
  chat_output: NODE_SIZE.chat_output,
  scope: { w: 320, h: 200 },
  speaker: NODE_SIZE.speaker,
  map: { w: 300, h: 220 },
  signal_map: { w: 400, h: 300 },
  readout: { w: 300, h: 160 },
  decoder_log: { w: 360, h: 200 },
  dmr_trunk: { w: 380, h: 240 },
  video: { w: 240, h: 200 },
  recorder: NODE_SIZE.recorder,
  network_export: NODE_SIZE.network_export,
  export: NODE_SIZE.export,
  scanner: NODE_SIZE.scanner,
};

/** Vertical space the shell's header takes, so ports are spread down the body only. Ports start
 * half a step below it: a handle sitting exactly on the boundary puts its label across the title
 * row, over the pin and remove buttons. */
export const HEADER_PX = 26;
/** Distance between stacked ports on one side of a face. */
export const PORT_STEP_PX = 22;
/** Where the first port sits, measured from the top of the face. */
export const PORT_TOP_PX = HEADER_PX + PORT_STEP_PX / 2;

/**
 * The resize floor for a node with these ports. Ports sit at `PORT_TOP_PX + PORT_STEP_PX × index`
 * down each side, so a radio with more streams than its kind's base minimum has rows for must
 * refuse to shrink past its lowest port — the shell clips its overflow, and a clipped port is a
 * wire that cannot be grabbed.
 */
export function nodeMinSize(kind: NodeKind, ports: readonly PortSpec[]): { w: number; h: number } {
  const base = NODE_MIN_SIZE[kind];
  const deepest = Math.max(
    ports.filter((port) => port.direction === "in").length,
    ports.filter((port) => port.direction === "out").length,
  );
  return { w: base.w, h: Math.max(base.h, PORT_TOP_PX + PORT_STEP_PX * deepest) };
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

/**
 * A stored workspace brought up to today's port table, returned unchanged when it already is.
 *
 * One shape needs it so far. A scanner used to *consume* a radio's IQ; it now drives the radio
 * through a control wire running the other way, because a device's left side is what is done to
 * it and the wire is the ownership. An edge naming `scanner.iq` names a port that no longer
 * exists — and the server validates the *whole* snapshot on every write, so one stale wire would
 * refuse every later write, including a node drag that has nothing to do with it.
 *
 * It runs where a workspace enters the client, not at render: an edit that read the old shape out
 * of the cache would write it straight back, and cutting the migrated wire would not stick.
 */
export function migrateSnapshot(snapshot: WorkspaceSnapshot): WorkspaceSnapshot {
  const graph = migrateGraph(snapshot.graph);
  return graph === snapshot.graph ? snapshot : { ...snapshot, graph };
}

function migrateGraph(graph: PatchGraph): PatchGraph {
  const scanners = new Set(
    graph.nodes.filter((node) => node.kind === "scanner").map((node) => node.id),
  );
  const edges = graph.edges ?? [];
  // The whole IQ family, not the bare name: a scanner has no IQ input of any stream today, so
  // every spelling of one is the old shape and would refuse writes just the same.
  const consumed = (edge: PatchEdge): boolean =>
    scanners.has(edge.to.node) && portStream("iq", edge.to.port) !== null;
  if (!edges.some(consumed)) {
    return graph;
  }
  const port = (reference: PortRef): string => `${reference.node}.${reference.port}`;
  const owned = new Set(
    edges
      .filter((edge) => edge.from.port === "control")
      .flatMap((edge) => [port(edge.from), port(edge.to)]),
  );
  const migrated: PatchEdge[] = [];
  for (const edge of edges) {
    if (!consumed(edge)) {
      migrated.push(edge);
      continue;
    }
    const flipped: PatchEdge = {
      from: { node: edge.to.node, port: "control" },
      to: { node: edge.from.node, port: "control" },
    };
    const ends = [port(flipped.from), port(flipped.to)];
    // Ownership is exclusive at both ends. A patch that fanned one scanner across two radios —
    // legal while the wire was a stream — keeps the first and loses the rest, which is the only
    // outcome the server would accept anyway.
    if (ends.some((end) => owned.has(end))) {
      continue;
    }
    for (const end of ends) {
      owned.add(end);
    }
    migrated.push(flipped);
  }
  return { ...graph, edges: migrated };
}

export const RACK_COLS = 12;
export const RACK_ROWS = 8;
export const RACK_DEFAULT = { w: 6, h: 4 } as const;

/** `crates/wire/src/workspace.rs`'s `MAX_NAME_LEN`. The server validates the whole snapshot on
 * every write, so a label built here from something unbounded — a recording's file name — has to
 * be cut to this or the next arrangement gesture is refused along with it. */
export const MAX_NAME_LEN = 64;

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
