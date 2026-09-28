import type { MapOptions } from "maplibre-gl";
import { recordEvent } from "../diagnostics";

export type MapStyle = Exclude<NonNullable<MapOptions["style"]>, string>;

export const BASEMAP_STYLE_URL = "https://tiles.openfreemap.org/styles/liberty";
export const BASEMAP_TIMEOUT_MS = 4_000;

export type BasemapKind = "pending" | "online" | "blank";

export function blankStyle(background: string): MapStyle {
  return {
    version: 8,
    sources: {},
    layers: [{ id: "backdrop", type: "background", paint: { "background-color": background } }],
  };
}

export async function fetchOnlineStyle(): Promise<MapStyle | null> {
  try {
    const response = await fetch(BASEMAP_STYLE_URL, {
      signal: AbortSignal.timeout(BASEMAP_TIMEOUT_MS),
    });
    if (!response.ok) {
      recordEvent("warn", "map", `basemap style: HTTP ${response.status}`);
      return null;
    }
    return (await response.json()) as MapStyle;
  } catch (error) {
    recordEvent("warn", "map", `basemap style: ${String(error)}`);
    return null;
  }
}

export function chooseBasemap(
  online: MapStyle | null,
  background: string,
): { kind: BasemapKind; style: MapStyle } {
  if (online !== null) {
    return { kind: "online", style: online };
  }
  return { kind: "blank", style: blankStyle(background) };
}
