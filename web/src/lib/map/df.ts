import type { GeoJSONSource, Map as MapLibreMap } from "maplibre-gl";
import { bearingDeg, greatCircleKm } from "../propagation";
import type { DfEstimate, DfStation, LatLon, NavTarget } from "../types";
import { setSourceData } from "./sources";

export const DF_SOURCES = {
  rays: "df-rays",
  estimate: "df-estimate",
  ellipse: "df-ellipse",
  stations: "df-stations",
  nav: "df-nav",
  target: "df-nav-target",
  bistatic: "df-bistatic",
  sites: "df-sites",
  fixes: "df-fix-ellipses",
  trails: "df-fix-trails",
  tracks: "df-tracks",
} as const;

export const DF_LAYERS = [
  "df-rays",
  "df-ellipse-fill",
  "df-ellipse-line",
  "df-bistatic",
  "df-fix-ellipses",
  "df-fix-trails",
  "df-tracks",
  "df-sites",
  "df-estimate",
  "df-stations",
  "df-nav",
  "df-nav-target",
] as const;

export const TRACK_COLOR = "#7fb2e0";
export const TRANSMITTER_COLOR = "#e0a458";
export const STATION_COLOR = "#b07de0";

const EARTH_RADIUS_M = 6_371_000;
export const RAY_LENGTH_M = 25_000;
export const ELLIPSE_POINTS = 48;
export const FIX_TRAIL_POINTS = 16;

export interface BearingRay {
  lat: number;
  lon: number;
  bearingDeg: number;
  confidence: number;
  sigmaDeg: number;
  ageMs: number;
}

export interface BistaticEchoes {
  receiver: LatLon;
  illuminator: LatLon;
  rangesKm: readonly number[];
}

export interface RadarSites {
  node: string;
  receiver: LatLon;
  transmitter: LatLon;
}

export interface RadarFixPoint {
  key: string;
  id: number;
  lat: number;
  lon: number;
  majorM: number;
  minorM: number;
  orientationDeg: number;
}

export interface DfOverlay {
  rays: readonly BearingRay[];
  maxAgeMs: number;
  estimate: DfEstimate | null;
  emitters: readonly DfEstimate[];
  nav: NavTarget | null;
  stations: readonly DfStation[];
  bistatic: readonly BistaticEchoes[];
  sites: readonly RadarSites[];
  tracks: readonly RadarFixPoint[];
  unplaced: readonly string[];
  from: LatLon | null;
}

export type FixTrails = ReadonlyMap<string, readonly [number, number][]>;

interface Collection<G, P> {
  type: "FeatureCollection";
  features: { type: "Feature"; geometry: G; properties: P }[];
}

type Line = { type: "LineString"; coordinates: [number, number][] };
type Point = { type: "Point"; coordinates: [number, number] };
type Polygon = { type: "Polygon"; coordinates: [number, number][][] };

function collection<G, P>(features: Collection<G, P>["features"]): Collection<G, P> {
  return { type: "FeatureCollection", features };
}

function point<P>(lat: number, lon: number, properties: P) {
  return {
    type: "Feature" as const,
    geometry: { type: "Point" as const, coordinates: [lon, lat] as [number, number] },
    properties,
  };
}

export function destination(
  lat: number,
  lon: number,
  degrees: number,
  distanceM: number,
): [number, number] {
  const bearing = (degrees * Math.PI) / 180;
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

export function rayCollection(
  rays: readonly BearingRay[],
  maxAgeMs: number,
): Collection<Line, { weight: number; sigma: number }> {
  return collection(
    rays
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
          sigma: ray.sigmaDeg,
        },
      })),
  );
}

export function estimateCollection(
  estimate: DfEstimate | null,
): Collection<Point, { converged: boolean }> {
  return collection(
    estimate === null ? [] : [point(estimate.lat, estimate.lon, { converged: estimate.converged })],
  );
}

export function ellipseRing(
  lat: number,
  lon: number,
  semiMajorM: number,
  semiMinorM: number,
  bearing: number,
): [number, number][] {
  const radians = (bearing * Math.PI) / 180;
  const ring: [number, number][] = [];
  for (let step = 0; step <= ELLIPSE_POINTS; step++) {
    const angle = (step / ELLIPSE_POINTS) * Math.PI * 2;
    const along = semiMajorM * Math.cos(angle);
    const across = semiMinorM * Math.sin(angle);
    const east = along * Math.sin(radians) + across * Math.cos(radians);
    const north = along * Math.cos(radians) - across * Math.sin(radians);
    ring.push(
      destination(lat, lon, (Math.atan2(east, north) * 180) / Math.PI, Math.hypot(east, north)),
    );
  }
  return ring;
}

function estimateRing(estimate: DfEstimate): [number, number][] {
  return ellipseRing(
    estimate.lat,
    estimate.lon,
    estimate.ellipse_major_m / 2,
    estimate.ellipse_minor_m / 2,
    estimate.ellipse_bearing_deg,
  );
}

function sameSpot(a: DfEstimate, b: DfEstimate): boolean {
  return a.lat === b.lat && a.lon === b.lon;
}

export function ellipseCollection(
  estimate: DfEstimate | null,
  emitters: readonly DfEstimate[] = [],
): Collection<Polygon, Record<string, never>> {
  const others = emitters.filter((entry) => estimate === null || !sameSpot(entry, estimate));
  return collection(
    [...(estimate === null ? [] : [estimate]), ...others].map((entry) => ({
      type: "Feature" as const,
      geometry: { type: "Polygon" as const, coordinates: [estimateRing(entry)] },
      properties: {},
    })),
  );
}

export function stationCollection(
  stations: readonly DfStation[],
): Collection<Point, { label: string }> {
  return collection(
    stations.map((station) => point(station.lat, station.lon, { label: station.station_id })),
  );
}

export function bistaticRing(set: BistaticEchoes, rangeKm: number): [number, number][] | null {
  const rangeM = rangeKm * 1_000;
  if (!(rangeM > 0)) {
    return null;
  }
  const from: [number, number] = [set.receiver.lat, set.receiver.lon];
  const to: [number, number] = [set.illuminator.lat, set.illuminator.lon];
  const baselineM = greatCircleKm(from, to) * 1_000;
  const axis = bearingDeg(from, to);
  const [centreLon, centreLat] = destination(from[0], from[1], axis, baselineM / 2);
  return ellipseRing(
    centreLat,
    centreLon,
    (baselineM + rangeM) / 2,
    Math.sqrt(rangeM * (rangeM + 2 * baselineM)) / 2,
    axis,
  );
}

export function bistaticCollection(
  sets: readonly BistaticEchoes[],
): Collection<Line, { rangeKm: number }> {
  const features = [];
  for (const set of sets) {
    for (const rangeKm of set.rangesKm) {
      const ring = bistaticRing(set, rangeKm);
      if (ring !== null) {
        features.push({
          type: "Feature" as const,
          geometry: { type: "LineString" as const, coordinates: ring },
          properties: { rangeKm },
        });
      }
    }
  }
  return collection(features);
}

export function navCollection(
  from: LatLon | null,
  nav: NavTarget | null,
): Collection<Line, { kind: string }> {
  if (from === null || nav === null) {
    return collection([]);
  }
  return collection([
    {
      type: "Feature",
      geometry: {
        type: "LineString",
        coordinates: [
          [from.lon, from.lat],
          [nav.lon, nav.lat],
        ],
      },
      properties: { kind: nav.kind },
    },
  ]);
}

export function navTargetCollection(nav: NavTarget | null): Collection<Point, { kind: string }> {
  return collection(nav === null ? [] : [point(nav.lat, nav.lon, { kind: nav.kind })]);
}

export function siteCollection(
  sites: readonly RadarSites[],
): Collection<Point, { role: "rx" | "tx" }> {
  return collection(
    sites.flatMap((site) => [
      point<{ role: "rx" | "tx" }>(site.receiver.lat, site.receiver.lon, { role: "rx" }),
      point<{ role: "rx" | "tx" }>(site.transmitter.lat, site.transmitter.lon, { role: "tx" }),
    ]),
  );
}

export function trackCollection(
  tracks: readonly RadarFixPoint[],
): Collection<Point, { id: number }> {
  return collection(tracks.map((track) => point(track.lat, track.lon, { id: track.id })));
}

export function fixEllipseCollection(
  tracks: readonly RadarFixPoint[],
): Collection<Polygon, { id: number }> {
  return collection(
    tracks
      .filter((track) => track.majorM > 0 && track.minorM > 0)
      .map((track) => ({
        type: "Feature" as const,
        geometry: {
          type: "Polygon" as const,
          coordinates: [
            ellipseRing(track.lat, track.lon, track.majorM, track.minorM, track.orientationDeg),
          ],
        },
        properties: { id: track.id },
      })),
  );
}

export function advanceFixTrails(
  trails: FixTrails,
  tracks: readonly RadarFixPoint[],
  limit = FIX_TRAIL_POINTS,
): Map<string, readonly [number, number][]> {
  const next = new Map<string, readonly [number, number][]>();
  for (const track of tracks) {
    const held = trails.get(track.key) ?? [];
    const last = held.at(-1);
    const moved = last === undefined || last[0] !== track.lon || last[1] !== track.lat;
    next.set(
      track.key,
      moved ? [...held, [track.lon, track.lat] as [number, number]].slice(-limit) : held,
    );
  }
  return next;
}

export function trailCollection(trails: FixTrails): Collection<Line, { key: string }> {
  return collection(
    [...trails]
      .filter(([, points]) => points.length > 1)
      .map(([key, points]) => ({
        type: "Feature" as const,
        geometry: { type: "LineString" as const, coordinates: [...points] },
        properties: { key },
      })),
  );
}

export interface OverlayCounts {
  bearings: number;
  echoes: number;
  tracks: number;
  unplaced: readonly string[];
}

export function overlayCounts(overlay: DfOverlay | undefined): OverlayCounts {
  if (overlay === undefined) {
    return { bearings: 0, echoes: 0, tracks: 0, unplaced: [] };
  }
  return {
    bearings: overlay.rays.filter((ray) => ray.ageMs <= overlay.maxAgeMs).length,
    echoes: overlay.bistatic.reduce(
      (sum, set) => sum + set.rangesKm.filter((km) => km > 0).length,
      0,
    ),
    tracks: overlay.tracks.length,
    unplaced: overlay.unplaced,
  };
}

const EMPTY = { type: "FeatureCollection", features: [] } as const;

function removeDfLayers(map: MapLibreMap): void {
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
}

function addBearingLayers(map: MapLibreMap, accent: string): void {
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
}

function addRadarLayers(map: MapLibreMap, accent: string): void {
  map.addLayer({
    id: "df-bistatic",
    type: "line",
    source: DF_SOURCES.bistatic,
    paint: {
      "line-color": TRACK_COLOR,
      "line-width": 1,
      "line-opacity": 0.7,
      "line-dasharray": [3, 2],
    },
  });
  map.addLayer({
    id: "df-fix-ellipses",
    type: "fill",
    source: DF_SOURCES.fixes,
    paint: { "fill-color": TRACK_COLOR, "fill-opacity": 0.15 },
  });
  map.addLayer({
    id: "df-fix-trails",
    type: "line",
    source: DF_SOURCES.trails,
    paint: { "line-color": TRACK_COLOR, "line-width": 1.5, "line-opacity": 0.6 },
  });
  map.addLayer({
    id: "df-tracks",
    type: "circle",
    source: DF_SOURCES.tracks,
    paint: {
      "circle-radius": 4,
      "circle-color": TRACK_COLOR,
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 1,
    },
  });
  map.addLayer({
    id: "df-sites",
    type: "circle",
    source: DF_SOURCES.sites,
    paint: {
      "circle-radius": 5,
      "circle-color": ["match", ["get", "role"], "tx", TRANSMITTER_COLOR, accent],
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 1.5,
    },
  });
}

function addFusionLayers(map: MapLibreMap, accent: string): void {
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
      "circle-color": STATION_COLOR,
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 1,
    },
  });
  map.addLayer({
    id: "df-nav",
    type: "line",
    source: DF_SOURCES.nav,
    paint: { "line-color": TRANSMITTER_COLOR, "line-width": 2, "line-dasharray": [2, 2] },
  });
  map.addLayer({
    id: "df-nav-target",
    type: "circle",
    source: DF_SOURCES.target,
    paint: {
      "circle-radius": 5,
      "circle-opacity": 0,
      "circle-stroke-color": TRANSMITTER_COLOR,
      "circle-stroke-width": 2,
    },
  });
}

export function installDfLayers(map: MapLibreMap, accent: string, enabled: boolean): void {
  removeDfLayers(map);
  if (!enabled) {
    return;
  }
  for (const id of Object.values(DF_SOURCES)) {
    map.addSource(id, { type: "geojson", data: EMPTY });
  }
  addBearingLayers(map, accent);
  addRadarLayers(map, accent);
  addFusionLayers(map, accent);
}

export function drawDfOverlay(map: MapLibreMap, overlay: DfOverlay, trails: FixTrails): void {
  const source = (id: string) => map.getSource<GeoJSONSource>(id);
  setSourceData(source(DF_SOURCES.rays), rayCollection(overlay.rays, overlay.maxAgeMs));
  setSourceData(source(DF_SOURCES.estimate), estimateCollection(overlay.estimate));
  setSourceData(source(DF_SOURCES.ellipse), ellipseCollection(overlay.estimate, overlay.emitters));
  setSourceData(source(DF_SOURCES.stations), stationCollection(overlay.stations));
  setSourceData(source(DF_SOURCES.bistatic), bistaticCollection(overlay.bistatic));
  setSourceData(source(DF_SOURCES.nav), navCollection(overlay.from, overlay.nav));
  setSourceData(source(DF_SOURCES.target), navTargetCollection(overlay.nav));
  setSourceData(source(DF_SOURCES.sites), siteCollection(overlay.sites));
  setSourceData(source(DF_SOURCES.tracks), trackCollection(overlay.tracks));
  setSourceData(source(DF_SOURCES.fixes), fixEllipseCollection(overlay.tracks));
  setSourceData(source(DF_SOURCES.trails), trailCollection(trails));
}
