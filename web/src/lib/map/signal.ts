import type { Map as MapLibreMap } from "maplibre-gl";
import { SIGNAL_MAX_DBFS, SIGNAL_MIN_DBFS } from "../signalSurvey";

export const SIGNAL_SOURCE = "signal-survey";
export const SIGNAL_LAYERS = ["signal-survey-heat", "signal-survey-points"] as const;
export const SIGNAL_GRADIENT =
  "linear-gradient(to right, #231942, #5e2b83, #b33f62, #ef8354, #f6d365)";

const EMPTY = { type: "FeatureCollection", features: [] } as const;

function removeSignalLayers(map: MapLibreMap): void {
  for (const layer of SIGNAL_LAYERS) {
    if (map.getLayer(layer) !== undefined) {
      map.removeLayer(layer);
    }
  }
  if (map.getSource(SIGNAL_SOURCE) !== undefined) {
    map.removeSource(SIGNAL_SOURCE);
  }
}

export function installSignalLayers(map: MapLibreMap, edge: string, enabled: boolean): void {
  removeSignalLayers(map);
  if (!enabled) {
    return;
  }
  map.addSource(SIGNAL_SOURCE, { type: "geojson", data: EMPTY });
  map.addLayer({
    id: SIGNAL_LAYERS[0],
    type: "heatmap",
    source: SIGNAL_SOURCE,
    maxzoom: 17,
    paint: {
      "heatmap-weight": [
        "interpolate",
        ["linear"],
        ["get", "level"],
        SIGNAL_MIN_DBFS,
        0.05,
        SIGNAL_MAX_DBFS,
        1,
      ],
      "heatmap-intensity": ["interpolate", ["linear"], ["zoom"], 0, 0.35, 15, 1.25],
      "heatmap-radius": ["interpolate", ["linear"], ["zoom"], 0, 3, 15, 20],
      "heatmap-opacity": ["interpolate", ["linear"], ["zoom"], 13, 0.8, 17, 0.2],
      "heatmap-color": [
        "interpolate",
        ["linear"],
        ["heatmap-density"],
        0,
        "rgba(35,25,66,0)",
        0.15,
        "#231942",
        0.35,
        "#5e2b83",
        0.55,
        "#b33f62",
        0.75,
        "#ef8354",
        1,
        "#f6d365",
      ],
    },
  });
  map.addLayer({
    id: SIGNAL_LAYERS[1],
    type: "circle",
    source: SIGNAL_SOURCE,
    minzoom: 13,
    paint: {
      "circle-radius": ["interpolate", ["linear"], ["zoom"], 13, 2, 17, 6],
      "circle-opacity": ["interpolate", ["linear"], ["zoom"], 13, 0, 15, 0.9],
      "circle-color": [
        "interpolate",
        ["linear"],
        ["get", "level"],
        SIGNAL_MIN_DBFS,
        "#231942",
        -95,
        "#5e2b83",
        -70,
        "#b33f62",
        -45,
        "#ef8354",
        SIGNAL_MAX_DBFS,
        "#f6d365",
      ],
      "circle-stroke-color": edge,
      "circle-stroke-width": 1,
    },
  });
}
