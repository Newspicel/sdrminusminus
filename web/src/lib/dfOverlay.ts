import type { DfNodeState } from "./df";
import type { BearingRay, DfOverlay } from "./map/df";
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

/// Everything the map draws for a set of direction finders: one ray per bearing anyone reported,
/// and the fused estimate and guidance of whichever of them has got that far.
export function dfOverlay(
  nodes: readonly string[],
  byNode: Readonly<Record<string, DfNodeState>>,
  now: number,
  from: { lat: number; lon: number } | null,
): DfOverlay | undefined {
  if (nodes.length === 0) {
    return undefined;
  }
  const rays: BearingRay[] = [];
  let estimate = null;
  let guidance = null;
  const stations = [];
  for (const node of nodes) {
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
    estimate ??= state.fusion?.estimate ?? null;
    guidance ??= state.fusion?.guidance ?? null;
    stations.push(...(state.fusion?.stations ?? []));
  }
  return { rays, maxAgeMs: BEARING_MAX_AGE_MS, estimate, guidance, stations, from };
}
