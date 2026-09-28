import labels from "../generated/labels.json";
import type { PatchEdge, PatchGraph, PatchNode, PortRef, PortSpec } from "../lib/types";
import { ARRAY_LANE_PORT, type GraphContext, nodeOf, portStream } from "./graph";

export const REFUSAL = labels.refusal;

export const STEER_PORT = "steer";
export const RADAR_TRUTH_PORT = "adsb";
export const EVENTS_PORT = "events";

const LANE_SOURCES: ReadonlySet<PatchNode["kind"]> = new Set(["device", "recording"]);
const BEARING_SOURCES: ReadonlySet<PatchNode["kind"]> = new Set(["df", "hunt", "event_filter"]);
const ADSB_CHANNEL = "adsb";

export interface ArrayWire {
  lane: number;
  port: string;
  source: PortRef;
}

function isArrayLane(graph: PatchGraph, edge: PatchEdge): boolean {
  return (
    portStream(ARRAY_LANE_PORT, edge.to.port) !== null &&
    nodeOf(graph, edge.to.node)?.kind === "array"
  );
}

export function lanesOf(graph: PatchGraph, array: string): ArrayWire[] {
  if (nodeOf(graph, array)?.kind !== "array") {
    return [];
  }
  const wires: ArrayWire[] = [];
  for (const edge of graph.edges ?? []) {
    const lane = edge.to.node === array ? portStream(ARRAY_LANE_PORT, edge.to.port) : null;
    if (lane !== null) {
      wires.push({ lane, port: edge.to.port, source: edge.from });
    }
  }
  return wires.toSorted((a, b) => a.lane - b.lane);
}

function holdingWire(graph: PatchGraph, from: PortRef): PatchEdge | undefined {
  return (graph.edges ?? []).find(
    (edge) =>
      edge.from.node === from.node && edge.from.port === from.port && isArrayLane(graph, edge),
  );
}

export function arrayHoldingLane(graph: PatchGraph, from: PortRef): string | null {
  return holdingWire(graph, from)?.to.node ?? null;
}

export function arrayLabel(graph: PatchGraph, array: string): string {
  return nodeOf(graph, array)?.label ?? REFUSAL.unnamed_array;
}

function laneRefusal(
  context: GraphContext,
  graph: PatchGraph,
  from: PortRef,
  to: PortRef,
): string | null {
  const source = nodeOf(graph, from.node);
  if (
    source === undefined ||
    !LANE_SOURCES.has(source.kind) ||
    portStream("iq", from.port) === null
  ) {
    return REFUSAL.array_lane;
  }
  const held = holdingWire(graph, from);
  if (held !== undefined) {
    return held.to.node === to.node && held.to.port === to.port
      ? null
      : REFUSAL.lane_taken.replace("{label}", arrayLabel(graph, held.to.node));
  }
  const sameRadio = lanesOf(graph, to.node).some((wire) => wire.source.node === from.node);
  const coherence = context.bound?.get(from.node)?.capabilities.coherence ?? null;
  return sameRadio && coherence === "none" ? REFUSAL.shared_clock : null;
}

function carriesAdsb(source: PatchNode | undefined): boolean {
  if (source?.kind === "channel") {
    return source.data.channel_type === ADSB_CHANNEL;
  }
  return source?.kind === "event_filter";
}

export function arrayRefusal(
  context: GraphContext,
  graph: PatchGraph,
  from: PortRef,
  to: PortRef,
  input: PortSpec,
): string | null {
  const target = nodeOf(graph, to.node);
  const source = nodeOf(graph, from.node);
  if (target?.kind === "array" && portStream(ARRAY_LANE_PORT, input.name) !== null) {
    return laneRefusal(context, graph, from, to);
  }
  if (target?.kind === "beamformer" && input.name === STEER_PORT) {
    return source?.kind === "df" && from.port === EVENTS_PORT ? null : REFUSAL.steer;
  }
  if (target?.kind === "passive_radar" && input.name === RADAR_TRUTH_PORT) {
    return carriesAdsb(source) ? null : REFUSAL.adsb;
  }
  if (target?.kind === "triangulation" && input.name === EVENTS_PORT) {
    return source !== undefined && BEARING_SOURCES.has(source.kind) ? null : REFUSAL.triangulation;
  }
  return null;
}
