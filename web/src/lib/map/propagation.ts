import type {
  DataDrivenPropertyValueSpecification,
  GeoJSONSource,
  Map as MapLibreMap,
} from "maplibre-gl";
import type { PropagationCell, PropagationPath } from "../propagation";
import type { IonosondeStation } from "../types";
import { unwrapTrail } from "./bounds";

export const PROPAGATION_SOURCES = {
  cells: "propagation-cells",
  paths: "propagation-paths",
  sondes: "propagation-sondes",
} as const;

export const PROPAGATION_LAYERS = [
  "propagation-heat",
  "propagation-paths",
  "propagation-muf",
  "propagation-muf-label",
  "propagation-sondes",
  "propagation-sonde-label",
] as const;

export const MUF_MIN_MHZ = 4;

export const MUF_MAX_MHZ = 40;

export type PropagationLayer = "activity" | "muf";

export interface PropagationOverlay {
  cells: readonly PropagationCell[];
  paths: readonly PropagationPath[];
  sondes: readonly IonosondeStation[];
  layer: PropagationLayer;
}

interface CellFeature {
  type: "Feature";
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: {
    grid: string;
    weight: number;
    decodes: number;
    muf: number;
    hasMuf: boolean;
  };
}

export interface CellCollection {
  type: "FeatureCollection";
  features: CellFeature[];
}

interface PathFeature {
  type: "Feature";
  geometry: { type: "LineString"; coordinates: [number, number][] };
  properties: { weight: number };
}

export interface PathCollection {
  type: "FeatureCollection";
  features: PathFeature[];
}

interface SondeFeature {
  type: "Feature";
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: { code: string; muf: number; label: string };
}

export interface SondeCollection {
  type: "FeatureCollection";
  features: SondeFeature[];
}

const EMPTY = { type: "FeatureCollection", features: [] } as const;

export function cellCollection(cells: readonly PropagationCell[]): CellCollection {
  return {
    type: "FeatureCollection",
    features: cells.map((cell) => ({
      type: "Feature",
      geometry: { type: "Point", coordinates: [cell.longitude, cell.latitude] },
      properties: {
        grid: cell.key,
        weight: cell.weight,
        decodes: cell.decodes,
        muf: cell.measuredMuf3000Mhz ?? 0,
        hasMuf: cell.measuredMuf3000Mhz !== null,
      },
    })),
  };
}

export function pathCollection(paths: readonly PropagationPath[]): PathCollection {
  return {
    type: "FeatureCollection",
    features: paths.map((path) => ({
      type: "Feature",
      geometry: {
        type: "LineString",
        coordinates: unwrapTrail(greatCircleLine(path.from, path.to)),
      },
      properties: { weight: path.weight },
    })),
  };
}

export function sondeCollection(stations: readonly IonosondeStation[]): SondeCollection {
  return {
    type: "FeatureCollection",
    features: stations.map((station) => ({
      type: "Feature",
      geometry: { type: "Point", coordinates: [station.longitude, station.latitude] },
      properties: {
        code: station.code,
        muf: station.muf3000_mhz,
        label: station.muf3000_mhz.toFixed(1),
      },
    })),
  };
}

const LINE_STEPS = 32;

export function greatCircleLine(
  from: readonly [number, number],
  to: readonly [number, number],
): [number, number][] {
  const points: [number, number][] = [];
  for (let step = 0; step <= LINE_STEPS; step += 1) {
    const [latitude, longitude] = interpolate(from, to, step / LINE_STEPS);
    points.push([longitude, latitude]);
  }
  return points;
}

function interpolate(
  from: readonly [number, number],
  to: readonly [number, number],
  fraction: number,
): [number, number] {
  const rad = Math.PI / 180;
  const [lat1, lon1] = [from[0] * rad, from[1] * rad];
  const [lat2, lon2] = [to[0] * rad, to[1] * rad];
  const sinLat = Math.sin((lat2 - lat1) / 2);
  const sinLon = Math.sin((lon2 - lon1) / 2);
  const d =
    2 *
    Math.asin(
      Math.min(1, Math.sqrt(sinLat * sinLat + Math.cos(lat1) * Math.cos(lat2) * sinLon * sinLon)),
    );
  if (d < 1e-9) {
    return [from[0], from[1]];
  }
  const a = Math.sin((1 - fraction) * d) / Math.sin(d);
  const b = Math.sin(fraction * d) / Math.sin(d);
  const x = a * Math.cos(lat1) * Math.cos(lon1) + b * Math.cos(lat2) * Math.cos(lon2);
  const y = a * Math.cos(lat1) * Math.sin(lon1) + b * Math.cos(lat2) * Math.sin(lon2);
  const z = a * Math.sin(lat1) + b * Math.sin(lat2);
  return [Math.atan2(z, Math.hypot(x, y)) / rad, Math.atan2(y, x) / rad];
}

export function updatePropagationSources(
  map: Pick<MapLibreMap, "getSource">,
  overlay: PropagationOverlay,
): { cells: CellCollection; paths: PathCollection; sondes: SondeCollection } {
  const cells = cellCollection(overlay.cells);
  const paths = pathCollection(overlay.paths);
  const sondes = sondeCollection(overlay.sondes);
  void map.getSource<GeoJSONSource>(PROPAGATION_SOURCES.cells)?.setData(cells);
  void map.getSource<GeoJSONSource>(PROPAGATION_SOURCES.paths)?.setData(paths);
  void map.getSource<GeoJSONSource>(PROPAGATION_SOURCES.sondes)?.setData(sondes);
  return { cells, paths, sondes };
}

export function installPropagationLayers(
  map: MapLibreMap,
  edge: string,
  accent: string,
  layer: PropagationLayer | null,
): void {
  for (const id of PROPAGATION_LAYERS) {
    if (map.getLayer(id) !== undefined) {
      map.removeLayer(id);
    }
  }
  for (const id of Object.values(PROPAGATION_SOURCES)) {
    if (map.getSource(id) !== undefined) {
      map.removeSource(id);
    }
  }
  if (layer === null) {
    return;
  }
  for (const id of Object.values(PROPAGATION_SOURCES)) {
    map.addSource(id, { type: "geojson", data: EMPTY });
  }

  map.addLayer({
    id: "propagation-paths",
    type: "line",
    source: PROPAGATION_SOURCES.paths,
    paint: {
      "line-color": accent,
      "line-width": 0.8,
      "line-opacity": ["interpolate", ["linear"], ["get", "weight"], 0, 0.05, 1, 0.45],
    },
  });

  if (layer === "activity") {
    map.addLayer({
      id: "propagation-heat",
      type: "heatmap",
      source: PROPAGATION_SOURCES.cells,
      paint: {
        "heatmap-weight": ["interpolate", ["linear"], ["get", "weight"], 0, 0.05, 12, 1],
        "heatmap-intensity": ["interpolate", ["linear"], ["zoom"], 0, 0.6, 8, 1.6],
        "heatmap-radius": ["interpolate", ["linear"], ["zoom"], 0, 12, 8, 48],
        "heatmap-opacity": 0.75,
        "heatmap-color": [
          "interpolate",
          ["linear"],
          ["heatmap-density"],
          0,
          "rgba(20,24,48,0)",
          0.15,
          "#1b2a5e",
          0.35,
          "#2f6f8f",
          0.55,
          "#3fae7a",
          0.75,
          "#e0a458",
          1,
          "#ef6262",
        ],
      },
    });
  } else {
    map.addLayer({
      id: "propagation-muf",
      type: "circle",
      source: PROPAGATION_SOURCES.cells,
      filter: ["==", ["get", "hasMuf"], true],
      paint: {
        "circle-radius": ["interpolate", ["linear"], ["zoom"], 0, 4, 6, 14],
        "circle-opacity": 0.85,
        "circle-color": mufRamp(),
        "circle-stroke-color": edge,
        "circle-stroke-width": 1,
      },
    });
    map.addLayer({
      id: "propagation-muf-label",
      type: "symbol",
      source: PROPAGATION_SOURCES.cells,
      filter: ["==", ["get", "hasMuf"], true],
      minzoom: 3,
      layout: {
        "text-field": ["number-format", ["get", "muf"], { "max-fraction-digits": 0 }],
        "text-font": ["Noto Sans Regular"],
        "text-size": 10,
        "text-allow-overlap": false,
        "text-optional": true,
      },
      paint: { "text-color": edge, "text-halo-color": "#ffffff", "text-halo-width": 1 },
    });
  }

  map.addLayer({
    id: "propagation-sondes",
    type: "circle",
    source: PROPAGATION_SOURCES.sondes,
    paint: {
      "circle-radius": 4,
      "circle-color": mufRamp(),
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 1.5,
    },
  });
  map.addLayer({
    id: "propagation-sonde-label",
    type: "symbol",
    source: PROPAGATION_SOURCES.sondes,
    minzoom: 3,
    layout: {
      "text-field": ["get", "label"],
      "text-font": ["Noto Sans Regular"],
      "text-size": 10,
      "text-anchor": "top",
      "text-offset": [0, 0.7],
      "text-optional": true,
    },
    paint: { "text-color": "#ffffff", "text-halo-color": edge, "text-halo-width": 1.2 },
  });
}

function mufRamp(): DataDrivenPropertyValueSpecification<string> {
  return [
    "interpolate",
    ["linear"],
    ["get", "muf"],
    MUF_MIN_MHZ,
    "#3b4a7a",
    10,
    "#2f6f8f",
    18,
    "#3fae7a",
    28,
    "#e0a458",
    MUF_MAX_MHZ,
    "#ef6262",
  ];
}

export function mufColor(mhz: number): string {
  const stops: readonly (readonly [number, string])[] = [
    [MUF_MIN_MHZ, "#3b4a7a"],
    [10, "#2f6f8f"],
    [18, "#3fae7a"],
    [28, "#e0a458"],
    [MUF_MAX_MHZ, "#ef6262"],
  ];
  const first = stops[0];
  const last = stops[stops.length - 1];
  if (first === undefined || last === undefined) {
    return "#3b4a7a";
  }
  if (mhz <= first[0]) {
    return first[1];
  }
  for (let index = 1; index < stops.length; index += 1) {
    const low = stops[index - 1];
    const high = stops[index];
    if (low === undefined || high === undefined || mhz > high[0]) {
      continue;
    }
    return mix(low[1], high[1], (mhz - low[0]) / (high[0] - low[0]));
  }
  return last[1];
}

function mix(from: string, to: string, fraction: number): string {
  const channels = [1, 3, 5].map((at) => {
    const a = Number.parseInt(from.slice(at, at + 2), 16);
    const b = Number.parseInt(to.slice(at, at + 2), 16);
    return Math.round(a + (b - a) * Math.min(1, Math.max(0, fraction)));
  });
  return `#${channels.map((value) => value.toString(16).padStart(2, "0")).join("")}`;
}
