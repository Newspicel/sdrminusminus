import type { Options } from "../../components/controls";
import type { FusionGridFrame } from "../../lib/frame";
import type {
  DfEstimate,
  DfFusionState,
  DfStation,
  FusionDecay,
  NavMode,
  NavTargetKind,
  PatchCatalog,
  PatchGraph,
  PatchNode,
  TriangulationParams,
} from "../../lib/types";
import { defaultBody } from "../newNode";

export const NAV_TEXT: Record<NavTargetKind, string> = {
  probe: "Drive across",
  estimate: "Drive at it",
};

export const NO_SOURCES = "Wire direction finders in";

const BEARING_KINDS: ReadonlySet<string> = new Set(["df", "hunt", "event_filter"]);

export type DecayKind = FusionDecay["kind"];

export const DECAY_OPTIONS: Options<DecayKind> = [
  { value: "auto", label: "Auto", title: "Fixed or moving, from the stations" },
  { value: "fixed", label: "Fixed", title: "Half life 1 min" },
  { value: "moving", label: "Moving", title: "Half life 30 min" },
  { value: "half_life", label: "Set", title: "Pick the half life" },
];

export const NAV_OPTIONS: Options<NavMode> = [
  { value: "auto", label: "Auto", title: "Cross the bearings first, then drive at the fix" },
  { value: "direct", label: "Direct", title: "Drive at the fix as soon as there is one" },
  { value: "off", label: "Off" },
];

export const DEFAULT_HALF_LIFE_S = 300;
export const MIN_HALF_LIFE_S = 10;
export const MAX_HALF_LIFE_S = 86_400;

export interface Box {
  w: number;
  h: number;
}

export type GridBounds = Pick<FusionGridFrame, "south" | "west" | "north" | "east">;

export function geoToGrid(
  point: { lat: number; lon: number },
  frame: GridBounds,
  box: Box,
): { x: number; y: number } {
  const width = frame.east - frame.west;
  const height = frame.north - frame.south;
  return {
    x: width === 0 ? box.w / 2 : ((point.lon - frame.west) / width) * box.w,
    y: height === 0 ? box.h / 2 : ((frame.north - point.lat) / height) * box.h,
  };
}

export function insideBox(point: { x: number; y: number }, box: Box): boolean {
  return point.x >= 0 && point.x <= box.w && point.y >= 0 && point.y <= box.h;
}

export function fusionSources(graph: PatchGraph, node: string): number {
  const sources = new Set<string>();
  for (const edge of graph.edges ?? []) {
    if (edge.to.node !== node || edge.to.port !== "events") {
      continue;
    }
    const from = graph.nodes.find((candidate) => candidate.id === edge.from.node);
    if (from !== undefined && BEARING_KINDS.has(from.kind)) {
      sources.add(from.id);
    }
  }
  return sources.size;
}

export function estimateLabel(estimate: DfEstimate | null): string {
  return estimate === null ? "-" : `${estimate.lat.toFixed(5)}, ${estimate.lon.toFixed(5)}`;
}

export function spreadLabel(estimate: DfEstimate | null): string {
  if (estimate === null) {
    return "-";
  }
  return `${metres(estimate.ellipse_major_m)} × ${metres(estimate.ellipse_minor_m)}`;
}

function metres(value: number): string {
  return value >= 1_000 ? `${(value / 1_000).toFixed(1)} km` : `${Math.round(value)} m`;
}

export interface GuidanceText {
  text: string;
  title?: string;
}

export function guidanceText(fusion: DfFusionState | undefined): GuidanceText {
  const nav = fusion?.nav ?? null;
  if (nav !== null) {
    return {
      text: `${NAV_TEXT[nav.kind]} · ${Math.round(nav.bearing_deg)}°`,
      title: `${metres(nav.distance_m)} away`,
    };
  }
  if (fusion?.no_guide_position === true) {
    return { text: "-", title: "Wire a GPS in to guide it" };
  }
  if (fusion?.no_bearings === true) {
    return { text: "-", title: "No bearings to steer by yet" };
  }
  return { text: "-" };
}

export function stationAge(station: DfStation, now: number): string {
  const seen = Date.parse(station.last_seen);
  if (Number.isNaN(seen)) {
    return "just now";
  }
  const seconds = Math.max(0, Math.round((now - seen) / 1_000));
  if (seconds < 5) {
    return "just now";
  }
  if (seconds < 90) {
    return `${seconds}s ago`;
  }
  return `${Math.round(seconds / 60)}m ago`;
}

export function stationBearing(station: DfStation): string {
  const deg = station.last_bearing_deg;
  return deg === undefined || !Number.isFinite(deg) ? "-" : `${deg.toFixed(1)}°`;
}

export function stationSigma(station: DfStation): string {
  const sigma = station.sigma_deg;
  return sigma === undefined || !Number.isFinite(sigma) || sigma <= 0
    ? "-"
    : `${sigma.toFixed(1)}°`;
}

export function decayWith(kind: DecayKind, current: FusionDecay): FusionDecay {
  if (kind !== "half_life") {
    return { kind };
  }
  return current.kind === "half_life"
    ? current
    : { kind: "half_life", seconds: DEFAULT_HALF_LIFE_S };
}

export function fadeTitle(fusion: DfFusionState | undefined): string | undefined {
  const seconds = fusion?.half_life_s ?? 0;
  return seconds > 0 ? `Old bearings count half after ${seconds} s` : undefined;
}

export function triangulationSettings(
  node: PatchNode,
  catalog: PatchCatalog,
): TriangulationParams | null {
  if (node.kind !== "triangulation") {
    return null;
  }
  if (node.data?.settings !== undefined) {
    return node.data.settings;
  }
  const body = defaultBody(catalog, "triangulation");
  return body?.kind === "triangulation" ? (body.data?.settings ?? null) : null;
}
