import type { GeoJSONSource, Map as MapLibreMap } from "maplibre-gl";
import { recordEvent } from "../diagnostics";
import { referenceCollection } from "./layers";
import { setSourceData } from "./sources";
import { ICON_SCALE, rasterize } from "./targets";

export const REFERENCE_ID = "station-reference";

const STATION_PX = 20;

function stationImage(color: string, edge: string): ImageData | null {
  return rasterize(STATION_PX, (ctx) => {
    const mid = STATION_PX / 2;
    ctx.beginPath();
    ctx.arc(mid, mid, 6, 0, Math.PI * 2);
    ctx.strokeStyle = edge;
    ctx.lineWidth = 3.5;
    ctx.stroke();
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.6;
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(mid, mid, 1.7, 0, Math.PI * 2);
    ctx.fillStyle = color;
    ctx.fill();
  });
}

export function installReferenceLayer(
  map: MapLibreMap,
  accent: string,
  edge: string,
  positions: readonly (readonly [number, number])[],
): void {
  if (map.getSource(REFERENCE_ID) !== undefined) {
    setSourceData(map.getSource<GeoJSONSource>(REFERENCE_ID), referenceCollection(positions));
    return;
  }
  if (!map.hasImage(REFERENCE_ID)) {
    const image = stationImage(accent, edge);
    if (image === null) {
      recordEvent("warn", "map", "reference icon failed");
      return;
    }
    map.addImage(REFERENCE_ID, image, { pixelRatio: ICON_SCALE });
  }
  map.addSource(REFERENCE_ID, { type: "geojson", data: referenceCollection(positions) });
  map.addLayer({
    id: REFERENCE_ID,
    type: "symbol",
    source: REFERENCE_ID,
    layout: {
      "icon-image": REFERENCE_ID,
      "icon-allow-overlap": true,
      "icon-ignore-placement": true,
    },
  });
}
