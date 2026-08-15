import type { GeoJSONSource, Map as MapLibreMap } from "maplibre-gl";
import type { PositionSample } from "../position";
import type { SignalSurveySample } from "../signalSurvey";
import { trailBounds, unwrapTrail } from "./bounds";
import type { TargetCollection } from "./layers";

interface PositionFeature {
  type: "Feature";
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: { latest: boolean; at: number };
}

export interface PositionCollection {
  type: "FeatureCollection";
  features: PositionFeature[];
}

export interface PositionRouteCollection {
  type: "FeatureCollection";
  features: {
    type: "Feature";
    geometry: { type: "LineString"; coordinates: [number, number][] };
    properties: Record<string, never>;
  }[];
}

export function positionCollection(
  tracks: readonly { samples: readonly PositionSample[]; active: boolean }[],
): {
  points: PositionCollection;
  route: PositionRouteCollection;
} {
  const features = tracks.flatMap(({ samples, active }) =>
    samples.map((sample, index) => ({
      type: "Feature" as const,
      geometry: {
        type: "Point" as const,
        coordinates: [sample.longitude, sample.latitude] as [number, number],
      },
      properties: { latest: active && index === samples.length - 1, at: sample.receivedAt },
    })),
  );
  return {
    points: { type: "FeatureCollection", features },
    route: {
      type: "FeatureCollection",
      features: tracks.flatMap(({ samples }) =>
        samples.length < 2
          ? []
          : [
              {
                type: "Feature" as const,
                geometry: {
                  type: "LineString" as const,
                  coordinates: unwrapTrail(
                    samples.map(
                      (sample) => [sample.longitude, sample.latitude] as [number, number],
                    ),
                  ),
                },
                properties: {},
              },
            ],
      ),
    },
  };
}

export function updatePositionSources(
  pointsSource: Pick<GeoJSONSource, "setData"> | undefined,
  routeSource: Pick<GeoJSONSource, "setData"> | undefined,
  tracks: readonly { samples: readonly PositionSample[]; active: boolean }[],
): { points: PositionCollection; route: PositionRouteCollection } {
  const collection = positionCollection(tracks);
  void pointsSource?.setData(collection.points);
  void routeSource?.setData(collection.route);
  return collection;
}

interface SignalFeature {
  type: "Feature";
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: { level: number; observations: number };
}

export interface SignalCollection {
  type: "FeatureCollection";
  features: SignalFeature[];
}

export function signalCollection(samples: readonly SignalSurveySample[]): SignalCollection {
  return {
    type: "FeatureCollection",
    features: samples.map((sample) => ({
      type: "Feature",
      geometry: { type: "Point", coordinates: [sample.longitude, sample.latitude] },
      properties: { level: sample.levelDbfs, observations: sample.observations },
    })),
  };
}

export function updateSignalSource(
  source: Pick<GeoJSONSource, "setData"> | undefined,
  samples: readonly SignalSurveySample[],
): SignalCollection {
  const collection = signalCollection(samples);
  void source?.setData(collection);
  return collection;
}

function frame(map: Pick<MapLibreMap, "fitBounds">, collection: TargetCollection): void {
  let west = Number.POSITIVE_INFINITY;
  let south = Number.POSITIVE_INFINITY;
  let east = Number.NEGATIVE_INFINITY;
  let north = Number.NEGATIVE_INFINITY;
  for (const feature of collection.features) {
    const [lon, lat] = feature.geometry.coordinates;
    west = Math.min(west, lon);
    east = Math.max(east, lon);
    south = Math.min(south, lat);
    north = Math.max(north, lat);
  }
  map.fitBounds(
    [
      [west, south],
      [east, north],
    ],
    { padding: 56, maxZoom: 9, duration: 0 },
  );
}

interface FrameFlag {
  current: boolean;
}

export function frameTargetsOnce(
  map: Pick<MapLibreMap, "fitBounds">,
  collection: TargetCollection,
  framed: FrameFlag,
): void {
  if (framed.current || collection.features.length === 0) {
    return;
  }
  framed.current = true;
  frame(map, collection);
}

export function framePositionOnce(
  map: Pick<MapLibreMap, "fitBounds">,
  collection: PositionCollection,
  framed: FrameFlag,
): void {
  if (framed.current || collection.features.length === 0) {
    return;
  }
  framed.current = true;
  framePoints(
    map,
    collection.features.map((feature) => feature.geometry.coordinates),
  );
}

export function frameSignalOnce(
  map: Pick<MapLibreMap, "fitBounds">,
  collection: SignalCollection,
  framed: FrameFlag,
): void {
  if (framed.current || collection.features.length === 0) {
    return;
  }
  framed.current = true;
  framePoints(
    map,
    collection.features.map((feature) => feature.geometry.coordinates),
  );
}

function framePoints(
  map: Pick<MapLibreMap, "fitBounds">,
  coordinates: readonly [number, number][],
): void {
  const bounds = trailBounds(coordinates);
  if (bounds === null) {
    return;
  }
  map.fitBounds(bounds, { padding: 56, maxZoom: 14, duration: 0 });
}
