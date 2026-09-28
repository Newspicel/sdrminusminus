import type { ImageSource, Map as MapLibreMap } from "maplibre-gl";
import { gridPixels } from "../../gl/surface";
import type { FusionGridFrame } from "../frame";

export const HEAT_SOURCE = "df-heat";
export const HEAT_LAYER = "df-heat";
export const HEAT_BELOW = "df-rays";
export const HEAT_OPACITY = 0.75;

const BLANK_PIXEL =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR42mNgAAIAAAUAAen63NgAAAAASUVORK5CYII=";
const EMPTY_BOX = 0.01;

export type HeatCorners = [[number, number], [number, number], [number, number], [number, number]];

export interface HeatScratch {
  pixels: Uint8ClampedArray<ArrayBuffer> | null;
}

export function heatCoordinates(
  frame: Pick<FusionGridFrame, "south" | "west" | "north" | "east">,
): HeatCorners {
  return [
    [frame.west, frame.north],
    [frame.east, frame.north],
    [frame.east, frame.south],
    [frame.west, frame.south],
  ];
}

export function installHeatLayer(map: MapLibreMap, enabled: boolean): void {
  if (map.getLayer(HEAT_LAYER) !== undefined) {
    map.removeLayer(HEAT_LAYER);
  }
  if (map.getSource(HEAT_SOURCE) !== undefined) {
    map.removeSource(HEAT_SOURCE);
  }
  if (!enabled) {
    return;
  }
  map.addSource(HEAT_SOURCE, {
    type: "image",
    url: BLANK_PIXEL,
    coordinates: heatCoordinates({ south: 0, west: 0, north: EMPTY_BOX, east: EMPTY_BOX }),
  });
  map.addLayer(
    {
      id: HEAT_LAYER,
      type: "raster",
      source: HEAT_SOURCE,
      paint: { "raster-opacity": HEAT_OPACITY, "raster-resampling": "nearest" },
    },
    map.getLayer(HEAT_BELOW) === undefined ? undefined : HEAT_BELOW,
  );
}

export function heatPixels(
  frame: FusionGridFrame,
  table: Uint8ClampedArray,
  scratch: HeatScratch,
): Uint8ClampedArray<ArrayBuffer> {
  const length = frame.cols * frame.rows * 4;
  const out =
    scratch.pixels !== null && scratch.pixels.length === length
      ? scratch.pixels
      : new Uint8ClampedArray(length);
  gridPixels(
    { cols: frame.cols, rows: frame.rows, cells: frame.cells },
    table,
    { flipY: false, transparentFloor: true },
    out,
  );
  scratch.pixels = out;
  return out;
}

export function drawHeat(
  map: MapLibreMap,
  frame: FusionGridFrame,
  table: Uint8ClampedArray,
  scratch: HeatScratch,
): void {
  const source = map.getSource<ImageSource>(HEAT_SOURCE);
  if (source === undefined || frame.cols === 0 || frame.rows === 0) {
    return;
  }
  const image = new ImageData(heatPixels(frame, table, scratch), frame.cols, frame.rows);
  source.updateImage({ image, coordinates: heatCoordinates(frame) });
}
