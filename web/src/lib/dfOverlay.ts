import type { DfOverlay } from "./map/df";
import type { DfFusionState, DfStation, PatchGraph } from "./types";

export const BEARING_MAX_AGE_MS = 5 * 60_000;

export function crossingSourcesOf(graph: PatchGraph, node: string): string[] {
  const kinds = new Map(graph.nodes.map((entry) => [entry.id, entry.kind]));
  return (graph.edges ?? [])
    .filter((edge) => edge.to.node === node && edge.to.port === "events")
    .map((edge) => edge.from.node)
    .filter((id) => kinds.get(id) === "triangulation");
}

export function dfOverlay(
  crossings: readonly string[],
  byNode: Readonly<Record<string, DfFusionState>>,
  from: { lat: number; lon: number } | null,
): DfOverlay | undefined {
  if (crossings.length === 0) {
    return undefined;
  }
  let estimate = null;
  let guidance = null;
  const stations: DfStation[] = [];
  for (const node of crossings) {
    const fusion = byNode[node];
    estimate ??= fusion?.estimate ?? null;
    guidance ??= fusion?.guidance ?? null;
    stations.push(...(fusion?.stations ?? []));
  }
  return {
    rays: [],
    maxAgeMs: BEARING_MAX_AGE_MS,
    estimate,
    guidance,
    stations,
    bistatic: [],
    from,
  };
}
