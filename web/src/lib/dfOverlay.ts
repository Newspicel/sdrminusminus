import type { DfNodeState } from "./df";
import type { BearingRay, BistaticEchoes, DfOverlay } from "./map/df";
import type { PatchGraph } from "./types";

/// How long a bearing stays on the map before it fades out entirely.
export const BEARING_MAX_AGE_MS = 5 * 60_000;

/// Which direction finders feed a display's events port.
export function dfSourcesOf(graph: PatchGraph, node: string): string[] {
  const kinds = new Map(graph.nodes.map((entry) => [entry.id, entry.kind]));
  return (graph.edges ?? [])
    .filter((edge) => edge.to.node === node && edge.to.port === "events")
    .map((edge) => edge.from.node)
    .filter((id) => kinds.get(id) === "df");
}

/// Which triangulation nodes feed a display's events port. Bearings cross there, so that is where
/// the estimate, its ellipse and the guidance come from.
export function crossingSourcesOf(graph: PatchGraph, node: string): string[] {
  const kinds = new Map(graph.nodes.map((entry) => [entry.id, entry.kind]));
  return (graph.edges ?? [])
    .filter((edge) => edge.to.node === node && edge.to.port === "events")
    .map((edge) => edge.from.node)
    .filter((id) => kinds.get(id) === "triangulation");
}

/// Where a finder's bearings are crossed: the triangulation nodes its events reach.
export function crossingsFedBy(graph: PatchGraph, node: string): string[] {
  const kinds = new Map(graph.nodes.map((entry) => [entry.id, entry.kind]));
  return (graph.edges ?? [])
    .filter((edge) => edge.from.node === node && edge.from.port === "events")
    .map((edge) => edge.to.node)
    .filter((id) => kinds.get(id) === "triangulation");
}

/// Which passive radars feed a display's events port, and where each one borrows its transmitter
/// from. A radar with no illuminator written down has nothing to draw an ellipse around.
export function radarSourcesOf(
  graph: PatchGraph,
  node: string,
): { node: string; illuminator: { lat: number; lon: number } }[] {
  const radars = new Map(
    graph.nodes
      .filter((entry) => entry.kind === "passive_radar")
      .map((entry) => [entry.id, entry.data.settings?.illuminator ?? null]),
  );
  return (graph.edges ?? [])
    .filter((edge) => edge.to.node === node && edge.to.port === "events")
    .flatMap((edge) => {
      const illuminator = radars.get(edge.from.node);
      return illuminator === undefined || illuminator === null
        ? []
        : [{ node: edge.from.node, illuminator: { lat: illuminator.lat, lon: illuminator.lon } }];
    });
}

export interface RadarSource {
  node: string;
  illuminator: { lat: number; lon: number };
}

export interface OverlaySources {
  finders: readonly string[];
  crossings?: readonly string[];
  radars?: readonly RadarSource[];
}

/// Everything the map draws: one ray per bearing any finder reported, the estimate and guidance of
/// wherever those bearings are crossed, and the ellipse each radar echo could have come off.
export function dfOverlay(
  sources: OverlaySources,
  byNode: Readonly<Record<string, DfNodeState>>,
  now: number,
  from: { lat: number; lon: number } | null,
): DfOverlay | undefined {
  const crossings = sources.crossings ?? [];
  const radars = sources.radars ?? [];
  if (sources.finders.length === 0 && crossings.length === 0 && radars.length === 0) {
    return undefined;
  }
  const rays: BearingRay[] = [];
  let estimate = null;
  let guidance = null;
  const stations = [];
  for (const node of sources.finders) {
    const state = byNode[node];
    if (state === undefined) {
      continue;
    }
    for (const sample of state.history) {
      const lat = sample.lat ?? from?.lat;
      const lon = sample.lon ?? from?.lon;
      if (lat === undefined || lon === undefined) {
        continue;
      }
      rays.push({
        lat,
        lon,
        bearingDeg: sample.bearingDeg,
        confidence: sample.confidence,
        ageMs: Math.max(0, now - sample.at),
      });
    }
  }
  for (const node of crossings) {
    const fusion = byNode[node]?.fusion;
    estimate ??= fusion?.estimate ?? null;
    guidance ??= fusion?.guidance ?? null;
    stations.push(...(fusion?.stations ?? []));
  }
  const bistatic: BistaticEchoes[] = [];
  for (const radar of radars) {
    const ranges = byNode[radar.node]?.detections ?? [];
    if (from === null || ranges.length === 0) {
      continue;
    }
    bistatic.push({
      receiver: from,
      illuminator: radar.illuminator,
      rangesKm: ranges.map((hit) => hit.range_km),
    });
  }
  return { rays, maxAgeMs: BEARING_MAX_AGE_MS, estimate, guidance, stations, bistatic, from };
}
