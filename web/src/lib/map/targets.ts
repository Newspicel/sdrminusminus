import type { Map as MapLibreMap, MapMouseEvent } from "maplibre-gl";
import { recordEvent } from "../diagnostics";
import {
  isTrackKind,
  KIND_STYLE,
  layerId,
  MAP_KINDS,
  type MapKind,
  sourceId,
  type TargetCollection,
  TRACK_KINDS,
  type TrackCollection,
  type TrackKind,
  trackSourceId,
} from "./layers";

export const HIT_SLOP_PX = 9;
export const ICON_SCALE = 2;

const ARROW_PX = 18;
const PLANE_PX = 26;
const SHIP_PX = 22;

const LAYER_PARTS = ["dot", "heading", "label"] as const;

const EMPTY_COLLECTION: TargetCollection = { type: "FeatureCollection", features: [] };

const EMPTY_TRACKS: TrackCollection = { type: "FeatureCollection", features: [] };

const LABEL_OFFSET_EM: Record<MapKind, number> = {
  adsb: 1.3,
  ais: 1.1,
  aprs: 0.7,
  radiosonde: 0.7,
};

export const LAYER_KIND: ReadonlyMap<string, MapKind> = new Map(
  MAP_KINDS.flatMap((kind) => LAYER_PARTS.map((part) => [layerId(kind, part), kind])),
);

export interface Selection {
  kind: MapKind;
  id: string;
}

export function hitTarget(map: MapLibreMap, event: MapMouseEvent): Selection | null {
  const layers = [...LAYER_KIND.keys()].filter((id) => map.getLayer(id) !== undefined);
  if (layers.length === 0) {
    return null;
  }
  const { x, y } = event.point;
  const hit = map.queryRenderedFeatures(
    [
      [x - HIT_SLOP_PX, y - HIT_SLOP_PX],
      [x + HIT_SLOP_PX, y + HIT_SLOP_PX],
    ],
    { layers },
  )[0];
  const kind = hit === undefined ? undefined : LAYER_KIND.get(hit.layer.id);
  const id: unknown = hit?.properties.id;
  return kind === undefined || typeof id !== "string" ? null : { kind, id };
}

function removeTargetLayers(map: MapLibreMap): void {
  for (const kind of MAP_KINDS) {
    for (const part of LAYER_PARTS) {
      if (map.getLayer(layerId(kind, part)) !== undefined) {
        map.removeLayer(layerId(kind, part));
      }
    }
    if (map.getSource(sourceId(kind)) !== undefined) {
      map.removeSource(sourceId(kind));
    }
  }
  for (const kind of TRACK_KINDS) {
    if (map.getLayer(layerId(kind, "track")) !== undefined) {
      map.removeLayer(layerId(kind, "track"));
    }
    if (map.getSource(trackSourceId(kind)) !== undefined) {
      map.removeSource(trackSourceId(kind));
    }
  }
}

function addTrackLayer(map: MapLibreMap, kind: TrackKind): void {
  map.addSource(trackSourceId(kind), { type: "geojson", data: EMPTY_TRACKS });
  map.addLayer({
    id: layerId(kind, "track"),
    type: "line",
    source: trackSourceId(kind),
    layout: { "line-join": "round", "line-cap": "round" },
    paint: { "line-color": KIND_STYLE[kind].color, "line-width": 1.5, "line-opacity": 0.7 },
  });
}

function addHeadingLayer(map: MapLibreMap, kind: MapKind, edge: string): boolean {
  const icon = `${sourceId(kind)}-icon`;
  if (!map.hasImage(icon)) {
    const image = KIND_ICON[kind](KIND_STYLE[kind].color, edge);
    if (image === null) {
      return false;
    }
    map.addImage(icon, image, { pixelRatio: ICON_SCALE });
  }
  map.addLayer({
    id: layerId(kind, "heading"),
    type: "symbol",
    source: sourceId(kind),
    filter: ["has", "heading"],
    layout: {
      "icon-image": icon,
      "icon-rotate": ["get", "heading"],
      "icon-rotation-alignment": "map",
      "icon-allow-overlap": true,
      "icon-ignore-placement": true,
    },
  });
  return true;
}

function addLabelLayer(map: MapLibreMap, kind: MapKind, edge: string): void {
  map.addLayer({
    id: layerId(kind, "label"),
    type: "symbol",
    source: sourceId(kind),
    layout: {
      "text-field": ["get", "label"],
      "text-font": ["Noto Sans Regular"],
      "text-size": 11,
      "text-anchor": "top",
      "text-offset": [0, LABEL_OFFSET_EM[kind]],
      "text-optional": true,
    },
    paint: {
      "text-color": KIND_STYLE[kind].color,
      "text-halo-color": edge,
      "text-halo-width": 1.4,
    },
  });
}

export function installTargetLayers(
  map: MapLibreMap,
  edge: string,
  kinds: readonly MapKind[],
): boolean {
  removeTargetLayers(map);
  let headings = true;
  for (const kind of kinds) {
    if (isTrackKind(kind)) {
      addTrackLayer(map, kind);
    }
  }
  for (const kind of kinds) {
    map.addSource(sourceId(kind), { type: "geojson", data: EMPTY_COLLECTION });
    map.addLayer({
      id: layerId(kind, "dot"),
      type: "circle",
      source: sourceId(kind),
      paint: {
        "circle-radius": 4,
        "circle-color": KIND_STYLE[kind].color,
        "circle-stroke-color": edge,
        "circle-stroke-width": 1,
      },
    });
    headings = addHeadingLayer(map, kind, edge) && headings;
  }
  for (const kind of kinds) {
    addLabelLayer(map, kind, edge);
  }
  if (!headings) {
    recordEvent("warn", "map", "heading icons failed");
  }
  return headings;
}

export function highlight(
  map: MapLibreMap | null,
  kinds: readonly MapKind[],
  selected: Selection | null,
): void {
  if (map === null) {
    return;
  }
  for (const kind of kinds) {
    if (map.getLayer(layerId(kind, "dot")) === undefined) {
      continue;
    }
    const active = selected !== null && selected.kind === kind ? selected.id : null;
    map.setPaintProperty(
      layerId(kind, "dot"),
      "circle-stroke-width",
      active === null ? 1 : ["case", ["==", ["get", "id"], active], 3, 1],
    );
    map.setPaintProperty(
      layerId(kind, "dot"),
      "circle-radius",
      active === null ? 4 : ["case", ["==", ["get", "id"], active], 6, 4],
    );
  }
}

export function rasterize(
  px: number,
  draw: (ctx: CanvasRenderingContext2D) => void,
): ImageData | null {
  const canvas = document.createElement("canvas");
  canvas.width = px * ICON_SCALE;
  canvas.height = px * ICON_SCALE;
  const ctx = canvas.getContext("2d");
  if (ctx === null) {
    return null;
  }
  ctx.scale(ICON_SCALE, ICON_SCALE);
  draw(ctx);
  return ctx.getImageData(0, 0, canvas.width, canvas.height);
}

function silhouette(
  ctx: CanvasRenderingContext2D,
  px: number,
  starboard: readonly (readonly [number, number])[],
): void {
  const outline = [...starboard, ...starboard.toReversed().map(([x, y]) => [px - x, y] as const)];
  ctx.beginPath();
  outline.forEach(([x, y], index) => (index === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)));
  ctx.closePath();
}

function paint(ctx: CanvasRenderingContext2D, color: string, edge: string): void {
  ctx.strokeStyle = edge;
  ctx.lineWidth = 1;
  ctx.lineJoin = "round";
  ctx.stroke();
  ctx.fillStyle = color;
  ctx.fill();
}

function arrowImage(color: string, edge: string): ImageData | null {
  return rasterize(ARROW_PX, (ctx) => {
    const mid = ARROW_PX / 2;
    ctx.beginPath();
    ctx.moveTo(mid, 1.5);
    ctx.lineTo(mid + 3.5, 8);
    ctx.lineTo(mid, 6.5);
    ctx.lineTo(mid - 3.5, 8);
    ctx.closePath();
    paint(ctx, color, edge);
  });
}

function planeImage(color: string, edge: string): ImageData | null {
  return rasterize(PLANE_PX, (ctx) => {
    silhouette(ctx, PLANE_PX, [
      [13, 1.6],
      [14.4, 4.2],
      [14.4, 9.4],
      [24.4, 14.2],
      [24.4, 16.2],
      [14.4, 13.6],
      [14.4, 19.2],
      [18.6, 21.8],
      [18.6, 23.4],
      [13.6, 22.4],
      [13, 23.6],
    ]);
    paint(ctx, color, edge);
  });
}

function shipImage(color: string, edge: string): ImageData | null {
  return rasterize(SHIP_PX, (ctx) => {
    silhouette(ctx, SHIP_PX, [
      [11, 1.8],
      [15.6, 7.4],
      [15.6, 17.2],
      [14.2, 19.8],
      [11, 19.8],
    ]);
    paint(ctx, color, edge);
  });
}

const KIND_ICON: Record<MapKind, (color: string, edge: string) => ImageData | null> = {
  adsb: planeImage,
  ais: shipImage,
  aprs: arrowImage,
  radiosonde: arrowImage,
};
