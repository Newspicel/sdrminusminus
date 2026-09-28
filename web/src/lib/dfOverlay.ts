import { eventSourcesOf } from "../canvas/binding";
import { nodeOf } from "../canvas/graph";
import { isStale as radarStale } from "../canvas/nodes/radar";
import { BEARING_MAX_AGE_MS, type BearingSample } from "./bearings";
import type { BearingRay, BistaticEchoes, DfOverlay, RadarFixPoint, RadarSites } from "./map/df";
import { type ProcessorState, readingOf } from "./processors";
import type { DfEstimate, DfFusionState, DfStation, LatLon, NavTarget, PatchGraph } from "./types";

export interface OverlaySources {
  finders: readonly string[];
  hunts: readonly string[];
  crossings: readonly string[];
  radars: readonly string[];
  labels: Readonly<Record<string, string>>;
}

type SourceKind = "df" | "hunt" | "triangulation" | "passive_radar";

const KIND_NAME: Readonly<Record<SourceKind, string>> = {
  df: "Direction finder",
  hunt: "Signal hunt",
  triangulation: "Triangulation",
  passive_radar: "Passive radar",
};

function isSourceKind(kind: string | undefined): kind is SourceKind {
  return kind !== undefined && kind in KIND_NAME;
}

export function overlaySourcesOf(graph: PatchGraph, map: string): OverlaySources {
  const found: Record<SourceKind, string[]> = {
    df: [],
    hunt: [],
    triangulation: [],
    passive_radar: [],
  };
  const labels: Record<string, string> = {};
  for (const id of eventSourcesOf(graph, map)) {
    const node = nodeOf(graph, id);
    if (node === undefined || !isSourceKind(node.kind)) {
      continue;
    }
    found[node.kind].push(id);
    labels[id] = node.label ?? KIND_NAME[node.kind];
  }
  return {
    finders: found.df,
    hunts: found.hunt,
    crossings: found.triangulation,
    radars: found.passive_radar,
    labels,
  };
}

function rayOf(sample: BearingSample, now: number): BearingRay {
  return {
    lat: sample.lat,
    lon: sample.lon,
    bearingDeg: sample.trueDeg,
    confidence: sample.confidence,
    sigmaDeg: sample.sigmaDeg,
    ageMs: Math.max(0, now - sample.at),
  };
}

function finderPlaced(state: ProcessorState | undefined): boolean {
  const reading = readingOf(state, "df");
  return reading === null || (reading.station != null && reading.peaks[0]?.true_deg != null);
}

interface RadarPart {
  sites: RadarSites[];
  bistatic: BistaticEchoes[];
  tracks: RadarFixPoint[];
  unplaced: string[];
}

function fixesOf(node: string, state: ProcessorState): RadarFixPoint[] {
  const update = readingOf(state, "passive_radar");
  return (update?.tracks ?? []).flatMap((track) =>
    track.fix == null
      ? []
      : [
          {
            key: `${node}:${track.id}`,
            id: track.id,
            lat: track.fix.lat,
            lon: track.fix.lon,
            majorM: track.fix.major_m,
            minorM: track.fix.minor_m,
            orientationDeg: track.fix.orientation_deg,
          },
        ],
  );
}

function radarPart(
  radars: readonly string[],
  labels: Readonly<Record<string, string>>,
  processors: Readonly<Record<string, ProcessorState>>,
  now: number,
): RadarPart {
  const part: RadarPart = { sites: [], bistatic: [], tracks: [], unplaced: [] };
  for (const node of radars) {
    const state = processors[node];
    const update = readingOf(state, "passive_radar");
    if (state === undefined || update === null) {
      continue;
    }
    const geometry = update.geometry;
    if (geometry == null) {
      part.unplaced.push(labels[node] ?? node);
      continue;
    }
    const receiver: LatLon = { lat: geometry.receiver.lat, lon: geometry.receiver.lon };
    const illuminator: LatLon = { lat: geometry.transmitter.lat, lon: geometry.transmitter.lon };
    part.sites.push({ node, receiver, transmitter: illuminator });
    if (radarStale(state.receivedAt, update, now)) {
      continue;
    }
    const rangesKm = update.tracks
      .filter((track) => track.state === "confirmed")
      .map((track) => track.range_km);
    part.bistatic.push({ receiver, illuminator, rangesKm });
    part.tracks.push(...fixesOf(node, state));
  }
  return part;
}

interface FusionPart {
  estimate: DfEstimate | null;
  emitters: DfEstimate[];
  nav: NavTarget | null;
  stations: DfStation[];
}

function fusionPart(
  crossings: readonly string[],
  fusion: Readonly<Record<string, DfFusionState>>,
): FusionPart {
  const part: FusionPart = { estimate: null, emitters: [], nav: null, stations: [] };
  for (const node of crossings) {
    const state = fusion[node];
    if (state === undefined) {
      continue;
    }
    if (part.estimate === null && state.estimate != null) {
      part.estimate = state.estimate;
      part.nav = state.nav ?? null;
    }
    part.emitters.push(...(state.emitters ?? []));
    part.stations.push(...(state.stations ?? []));
  }
  return part;
}

export function dfOverlay(
  sources: OverlaySources,
  bearings: Readonly<Record<string, readonly BearingSample[]>>,
  fusion: Readonly<Record<string, DfFusionState>>,
  processors: Readonly<Record<string, ProcessorState>>,
  now: number,
  from: LatLon | null,
): DfOverlay | undefined {
  const { finders, hunts, crossings, radars, labels } = sources;
  if (finders.length + hunts.length + crossings.length + radars.length === 0) {
    return undefined;
  }
  const rays = [...finders, ...hunts].flatMap((node) =>
    (bearings[node] ?? []).map((sample) => rayOf(sample, now)),
  );
  const radar = radarPart(radars, labels, processors, now);
  const unplacedFinders = finders
    .filter((node) => !finderPlaced(processors[node]))
    .map((node) => labels[node] ?? node);
  return {
    rays,
    maxAgeMs: BEARING_MAX_AGE_MS,
    ...fusionPart(crossings, fusion),
    bistatic: radar.bistatic,
    sites: radar.sites,
    tracks: radar.tracks,
    unplaced: [...unplacedFinders, ...radar.unplaced],
    from,
  };
}
