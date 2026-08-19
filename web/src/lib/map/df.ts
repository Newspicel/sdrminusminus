import type { GeoJSONSource, Map as MapLibreMap } from "maplibre-gl";
import { bearingDeg, greatCircleKm } from "../propagation";
import type { DfEstimate, DfGuidance, DfStation } from "../types";

export const DF_SOURCES = {
  rays: "df-rays",
  estimate: "df-estimate",
  ellipse: "df-ellipse",
  stations: "df-stations",
  nav: "df-nav",
  bistatic: "df-bistatic",
} as const;

export const DF_LAYERS = [
  "df-rays",
  "df-ellipse-fill",
  "df-ellipse-line",
  "df-estimate",
  "df-stations",
  "df-nav",
  "df-bistatic",
] as const;

const EARTH_RADIUS_M = 6_371_000;
/// How far a bearing ray is drawn. Long enough to cross a town, short enough that a wrong bearing
/// does not sweep the whole map.
export const RAY_LENGTH_M = 25_000;
export const ELLIPSE_POINTS = 48;

export interface BearingRay {
  lat: number;
  lon: number;
  bearingDeg: number;
  confidence: number;
  ageMs: number;
}

interface Collection<G, P> {
  type: "FeatureCollection";
  features: { type: "Feature"; geometry: G; properties: P }[];
}

type Line = { type: "LineString"; coordinates: [number, number][] };
type Point = { type: "Point"; coordinates: [number, number] };
type Polygon = { type: "Polygon"; coordinates: [number, number][][] };

export function destination(
  lat: number,
  lon: number,
  bearingDeg: number,
  distanceM: number,
): [number, number] {
  const bearing = (bearingDeg * Math.PI) / 180;
  const angular = distanceM / EARTH_RADIUS_M;
  const phi = (lat * Math.PI) / 180;
  const lambda = (lon * Math.PI) / 180;
  const sinPhi =
    Math.sin(phi) * Math.cos(angular) + Math.cos(phi) * Math.sin(angular) * Math.cos(bearing);
  const phi2 = Math.asin(Math.min(1, Math.max(-1, sinPhi)));
  const lambda2 =
    lambda +
    Math.atan2(
      Math.sin(bearing) * Math.sin(angular) * Math.cos(phi),
      Math.cos(angular) - Math.sin(phi) * sinPhi,
    );
  return [(((lambda2 * 180) / Math.PI + 540) % 360) - 180, (phi2 * 180) / Math.PI];
}

/// One line per bearing, newest fully opaque and older ones fading out, so a trail of readings
/// reads as a trail rather than a fan of equals.
export function rayCollection(
  rays: readonly BearingRay[],
  maxAgeMs: number,
): Collection<Line, { weight: number }> {
  const features = rays
    .filter((ray) => ray.ageMs <= maxAgeMs)
    .map((ray) => ({
      type: "Feature" as const,
      geometry: {
        type: "LineString" as const,
        coordinates: [
          [ray.lon, ray.lat] as [number, number],
          destination(ray.lat, ray.lon, ray.bearingDeg, RAY_LENGTH_M),
        ],
      },
      properties: {
        weight: Math.max(0.05, ray.confidence * (1 - ray.ageMs / Math.max(1, maxAgeMs))),
      },
    }));
  return { type: "FeatureCollection", features };
}

export function estimateCollection(
  estimate: DfEstimate | null,
): Collection<Point, { converged: boolean }> {
  return {
    type: "FeatureCollection",
    features:
      estimate === null
        ? []
        : [
            {
              type: "Feature",
              geometry: { type: "Point", coordinates: [estimate.lon, estimate.lat] },
              properties: { converged: estimate.converged },
            },
          ],
  };
}

/// The uncertainty ellipse as a ring on the ground: its long axis points along the bearing the
/// estimate is least sure about, which is what tells an operator which way to drive.
export function ellipseCollection(
  estimate: DfEstimate | null,
): Collection<Polygon, Record<string, never>> {
  if (estimate === null) {
    return { type: "FeatureCollection", features: [] };
  }
  const ring: [number, number][] = [];
  for (let step = 0; step <= ELLIPSE_POINTS; step++) {
    const angle = (step / ELLIPSE_POINTS) * Math.PI * 2;
    const along = (estimate.ellipse_major_m / 2) * Math.cos(angle);
    const across = (estimate.ellipse_minor_m / 2) * Math.sin(angle);
    const bearing = estimate.ellipse_bearing_deg;
    const east =
      along * Math.sin((bearing * Math.PI) / 180) + across * Math.cos((bearing * Math.PI) / 180);
    const north =
      along * Math.cos((bearing * Math.PI) / 180) - across * Math.sin((bearing * Math.PI) / 180);
    const distance = Math.hypot(east, north);
    const direction = (Math.atan2(east, north) * 180) / Math.PI;
    ring.push(destination(estimate.lat, estimate.lon, direction, distance));
  }
  return {
    type: "FeatureCollection",
    features: [
      {
        type: "Feature",
        geometry: { type: "Polygon", coordinates: [ring] },
        properties: {},
      },
    ],
  };
}

export function stationCollection(
  stations: readonly DfStation[],
): Collection<Point, { label: string }> {
  return {
    type: "FeatureCollection",
    features: stations.map((station) => ({
      type: "Feature" as const,
      geometry: { type: "Point" as const, coordinates: [station.lon, station.lat] },
      properties: { label: station.station_id },
    })),
  };
}

export interface BistaticEchoes {
  receiver: { lat: number; lon: number };
  illuminator: { lat: number; lon: number };
  rangesKm: readonly number[];
}

/// Everything one echo could have bounced off. A passive radar measures how much further the echo
/// travelled than the direct path, which puts the reflector somewhere on the ellipse with the
/// transmitter and the receiver at its foci — not on a bearing, and not at a point.
export function bistaticRing(set: BistaticEchoes, rangeKm: number): [number, number][] | null {
  const rangeM = rangeKm * 1_000;
  if (!(rangeM > 0)) {
    return null;
  }
  const from: [number, number] = [set.receiver.lat, set.receiver.lon];
  const to: [number, number] = [set.illuminator.lat, set.illuminator.lon];
  const baselineM = greatCircleKm(from, to) * 1_000;
  const along = (baselineM + rangeM) / 2;
  const across = Math.sqrt(rangeM * (rangeM + 2 * baselineM)) / 2;
  const axis = bearingDeg(from, to);
  const [centreLon, centreLat] = destination(from[0], from[1], axis, baselineM / 2);
  const ring: [number, number][] = [];
  for (let step = 0; step <= ELLIPSE_POINTS; step++) {
    const angle = (step / ELLIPSE_POINTS) * Math.PI * 2;
    const forward = along * Math.cos(angle);
    const sideways = across * Math.sin(angle);
    const radians = (axis * Math.PI) / 180;
    const east = forward * Math.sin(radians) + sideways * Math.cos(radians);
    const north = forward * Math.cos(radians) - sideways * Math.sin(radians);
    ring.push(
      destination(
        centreLat,
        centreLon,
        (Math.atan2(east, north) * 180) / Math.PI,
        Math.hypot(east, north),
      ),
    );
  }
  return ring;
}

export function bistaticCollection(
  sets: readonly BistaticEchoes[],
): Collection<Line, { rangeKm: number }> {
  const features = [];
  for (const set of sets) {
    for (const rangeKm of set.rangesKm) {
      const ring = bistaticRing(set, rangeKm);
      if (ring === null) {
        continue;
      }
      features.push({
        type: "Feature" as const,
        geometry: { type: "LineString" as const, coordinates: ring },
        properties: { rangeKm },
      });
    }
  }
  return { type: "FeatureCollection", features };
}

export function navCollection(
  from: { lat: number; lon: number } | null,
  guidance: DfGuidance | null,
): Collection<Line, { kind: string }> {
  if (from === null || guidance === null) {
    return { type: "FeatureCollection", features: [] };
  }
  return {
    type: "FeatureCollection",
    features: [
      {
        type: "Feature",
        geometry: {
          type: "LineString",
          coordinates: [
            [from.lon, from.lat],
            [guidance.nav_target.lon, guidance.nav_target.lat],
          ],
        },
        properties: { kind: guidance.nav_target.kind },
      },
    ],
  };
}

export interface DfOverlay {
  rays: readonly BearingRay[];
  maxAgeMs: number;
  estimate: DfEstimate | null;
  guidance: DfGuidance | null;
  stations: readonly DfStation[];
  bistatic: readonly BistaticEchoes[];
  from: { lat: number; lon: number } | null;
}

const EMPTY = { type: "FeatureCollection", features: [] } as const;

export function installDfLayers(map: MapLibreMap, accent: string, enabled: boolean): void {
  for (const id of DF_LAYERS) {
    if (map.getLayer(id) !== undefined) {
      map.removeLayer(id);
    }
  }
  for (const id of Object.values(DF_SOURCES)) {
    if (map.getSource(id) !== undefined) {
      map.removeSource(id);
    }
  }
  if (!enabled) {
    return;
  }
  for (const id of Object.values(DF_SOURCES)) {
    map.addSource(id, { type: "geojson", data: EMPTY });
  }
  map.addLayer({
    id: "df-rays",
    type: "line",
    source: DF_SOURCES.rays,
    paint: {
      "line-color": accent,
      "line-width": 1.5,
      "line-opacity": ["interpolate", ["linear"], ["get", "weight"], 0, 0.08, 1, 0.9],
    },
  });
  map.addLayer({
    id: "df-ellipse-fill",
    type: "fill",
    source: DF_SOURCES.ellipse,
    paint: { "fill-color": accent, "fill-opacity": 0.12 },
  });
  map.addLayer({
    id: "df-ellipse-line",
    type: "line",
    source: DF_SOURCES.ellipse,
    paint: { "line-color": accent, "line-width": 1, "line-opacity": 0.6 },
  });
  map.addLayer({
    id: "df-bistatic",
    type: "line",
    source: DF_SOURCES.bistatic,
    paint: {
      "line-color": "#7fb2e0",
      "line-width": 1,
      "line-opacity": 0.7,
      "line-dasharray": [3, 2],
    },
  });
  map.addLayer({
    id: "df-nav",
    type: "line",
    source: DF_SOURCES.nav,
    paint: {
      "line-color": "#e0a458",
      "line-width": 2,
      "line-dasharray": [2, 2],
    },
  });
  map.addLayer({
    id: "df-estimate",
    type: "circle",
    source: DF_SOURCES.estimate,
    paint: {
      "circle-radius": 6,
      "circle-color": ["case", ["get", "converged"], "#3fae7a", accent],
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 1.5,
    },
  });
  map.addLayer({
    id: "df-stations",
    type: "circle",
    source: DF_SOURCES.stations,
    paint: {
      "circle-radius": 4,
      "circle-color": "#b07de0",
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 1,
    },
  });
}

export function drawDfOverlay(map: MapLibreMap, overlay: DfOverlay): void {
  void map
    .getSource<GeoJSONSource>(DF_SOURCES.rays)
    ?.setData(rayCollection(overlay.rays, overlay.maxAgeMs));
  void map
    .getSource<GeoJSONSource>(DF_SOURCES.estimate)
    ?.setData(estimateCollection(overlay.estimate));
  void map
    .getSource<GeoJSONSource>(DF_SOURCES.ellipse)
    ?.setData(ellipseCollection(overlay.estimate));
  void map
    .getSource<GeoJSONSource>(DF_SOURCES.stations)
    ?.setData(stationCollection(overlay.stations));
  void map
    .getSource<GeoJSONSource>(DF_SOURCES.bistatic)
    ?.setData(bistaticCollection(overlay.bistatic));
  void map
    .getSource<GeoJSONSource>(DF_SOURCES.nav)
    ?.setData(navCollection(overlay.from, overlay.guidance));
}
