import { rateMismatch } from "../components/channelSettings";
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

export interface GraphContext {
  catalog: PatchCatalog;
  channelTypes: readonly ChannelDescriptor[];
  bound?: ReadonlyMap<string, DeviceSet>;
}

export function nodeOf(graph: PatchGraph, id: string): PatchNode | undefined {
  return graph.nodes.find((node) => node.id === id);
}

export const MAX_STREAMS = 16;

export function streamPort(base: string, index: number): string {
  return index === 0 ? base : `${base}${index + 1}`;
}

export function portLabel(name: string, siblings: readonly PortSpec[]): string {
  const numbered = `${name}2`;
  return siblings.some((port) => port.name === numbered) ? `${name}1` : name;
}

export function streamLabel(base: string, index: number, streams: number): string {
  return streams > 1 && index === 0 ? `${base}1` : streamPort(base, index);
}

export function rxStreamCount(capabilities: Capabilities | undefined): number {
  return clampStreams(capabilities?.rx_streams);
}

function clampStreams(declared: number | undefined): number {
  return Math.min(Math.max(declared ?? 1, 1), MAX_STREAMS);
}

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

export const MAX_NODES = 128;
export const MAX_EDGES = 256;

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
          return hasTransmitter(capabilities?.duplex);
        default:
          return false;
      }
    })
    .flatMap((port) => expandStreams(port, node.id, graph.edges ?? [], capabilities));
}

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
  return wanted.min === wanted.max
    ? `needs ${mhz(wanted.min)} MHz`
    : `needs ${mhz(wanted.min)}–${mhz(wanted.max)} MHz`;
}

function mhz(hz: number): string {
  return (hz / 1e6).toFixed(3);
}

export interface NodeSize {
  w: number;
  h?: number;
}

export const NODE_SIZE: Record<NodeKind, NodeSize> = {
  device: { w: 380 },
  gps: { w: 360 },
  channel: { w: 440 },
  chat_output: { w: 420 },
  event_filter: { w: 380 },
  scope: { w: 520, h: 360 },
  speaker: { w: 320 },
  map: { w: 520, h: 380 },
  signal_map: { w: 600, h: 440 },
  propagation: { w: 640, h: 560 },
  readout: { w: 560, h: 320 },
  decoder_log: { w: 720, h: 380 },
  dmr_trunk: { w: 480, h: 360 },
  video: { w: 380, h: 320 },
  recorder: { w: 340 },
  audio_recorder: { w: 340 },
  baseband_recorder: { w: 340 },
  time_machine: { w: 360 },
  network_export: { w: 380 },
  export: { w: 320 },
  scanner: { w: 400 },
};

export function isResizable(kind: NodeKind): boolean {
  return NODE_SIZE[kind].h !== undefined;
}

const RESIZE_FLOOR: Partial<Record<NodeKind, { w: number; h: number }>> = {
  scope: { w: 320, h: 200 },
  map: { w: 300, h: 220 },
  signal_map: { w: 400, h: 300 },
  propagation: { w: 440, h: 380 },
  readout: { w: 300, h: 160 },
  decoder_log: { w: 360, h: 200 },
  dmr_trunk: { w: 380, h: 240 },
  video: { w: 240, h: 200 },
};

export const HEADER_PX = 26;
export const PORT_STEP_PX = 22;
export const PORT_TOP_PX = HEADER_PX + PORT_STEP_PX / 2;

export function nodeMinSize(kind: NodeKind, ports: readonly PortSpec[]): { w: number; h: number } {
  const base = RESIZE_FLOOR[kind] ?? { w: NODE_SIZE[kind].w, h: 0 };
  const deepest = Math.max(
    ports.filter((port) => port.direction === "in").length,
    ports.filter((port) => port.direction === "out").length,
  );
  return { w: base.w, h: Math.max(base.h, PORT_TOP_PX + PORT_STEP_PX * deepest) };
}

export function addNode(graph: PatchGraph, node: PatchNode): PatchGraph {
  return { ...graph, nodes: [...graph.nodes, node] };
}

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

export function sameGraph(a: PatchGraph, b: PatchGraph): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

export function migrateSnapshot(snapshot: WorkspaceSnapshot): WorkspaceSnapshot {
  const graph = migrateGraph(snapshot.graph);
  return graph === snapshot.graph ? snapshot : { ...snapshot, graph };
}

function migrateGraph(graph: PatchGraph): PatchGraph {
  const scanners = new Set(
    graph.nodes.filter((node) => node.kind === "scanner").map((node) => node.id),
  );
  const edges = graph.edges ?? [];
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

export const MAX_NAME_LEN = 64;

export interface RackCell {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type RackEdge = "n" | "e" | "s" | "w";

export function isPinned(rack: RackLayout, node: string): boolean {
  return (rack.slots ?? []).some((slot) => slot.node === node);
}

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

export function placeSlot(rack: RackLayout, node: string, cell: RackCell): RackLayout {
  const slots = rack.slots ?? [];
  if (!inside(cell) || !slots.every((slot) => slot.node === node || !overlaps(slot, cell))) {
    return rack;
  }
  return { slots: slots.map((slot) => (slot.node === node ? { node, ...cell } : slot)) };
}

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
