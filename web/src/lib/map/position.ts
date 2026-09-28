import type { Map as MapLibreMap } from "maplibre-gl";

export const POSITION_SOURCE = "station-position-history";
export const POSITION_ROUTE_SOURCE = "station-position-route";
export const POSITION_LAYERS = [
  "station-position-heat",
  "station-position-route",
  "station-position-fix",
] as const;

const EMPTY = { type: "FeatureCollection", features: [] } as const;

function removePositionLayers(map: MapLibreMap): void {
  for (const layer of POSITION_LAYERS) {
    if (map.getLayer(layer) !== undefined) {
      map.removeLayer(layer);
    }
  }
  for (const source of [POSITION_SOURCE, POSITION_ROUTE_SOURCE]) {
    if (map.getSource(source) !== undefined) {
      map.removeSource(source);
    }
  }
}

export function installPositionLayers(
  map: MapLibreMap,
  accent: string,
  edge: string,
  enabled: boolean,
): void {
  removePositionLayers(map);
  if (!enabled) {
    return;
  }
  map.addSource(POSITION_SOURCE, { type: "geojson", data: EMPTY });
  map.addSource(POSITION_ROUTE_SOURCE, { type: "geojson", data: EMPTY });
  map.addLayer({
    id: POSITION_LAYERS[0],
    type: "heatmap",
    source: POSITION_SOURCE,
    maxzoom: 16,
    paint: {
      "heatmap-weight": 1,
      "heatmap-intensity": ["interpolate", ["linear"], ["zoom"], 0, 0.5, 14, 2],
      "heatmap-radius": ["interpolate", ["linear"], ["zoom"], 0, 3, 14, 22],
      "heatmap-opacity": ["interpolate", ["linear"], ["zoom"], 10, 0.65, 16, 0.2],
      "heatmap-color": [
        "interpolate",
        ["linear"],
        ["heatmap-density"],
        0,
        "rgba(0,0,0,0)",
        0.35,
        accent,
        1,
        "#ef6262",
      ],
    },
  });
  map.addLayer({
    id: POSITION_LAYERS[1],
    type: "line",
    source: POSITION_ROUTE_SOURCE,
    paint: { "line-color": accent, "line-width": 2, "line-opacity": 0.8 },
  });
  map.addLayer({
    id: POSITION_LAYERS[2],
    type: "circle",
    source: POSITION_SOURCE,
    filter: ["==", ["get", "latest"], true],
    paint: {
      "circle-radius": 6,
      "circle-color": accent,
      "circle-stroke-color": edge,
      "circle-stroke-width": 2,
    },
  });
}
