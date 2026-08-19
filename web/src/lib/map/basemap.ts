import type { MapOptions } from "maplibre-gl";

export type MapStyle = Exclude<NonNullable<MapOptions["style"]>, string>;

export const BASEMAP_STYLE_URL = "https://tiles.openfreemap.org/styles/liberty";
export const OFFLINE_BASEMAP_URL = "/api/basemap.pmtiles";
export const BASEMAP_TIMEOUT_MS = 4_000;

export type BasemapKind = "pending" | "online" | "offline" | "blank";

/// A backdrop with nothing on it, for a station with neither internet nor an archive.
export function blankStyle(background: string): MapStyle {
  return {
    version: 8,
    sources: {},
    layers: [{ id: "backdrop", type: "background", paint: { "background-color": background } }],
  };
}

/// A small self-authored style over the operator's own archive.
///
/// Deliberately not a copy of a vendor style: it names only the layers every OpenMapTiles-schema
/// extract has, so any archive cut from the usual sources draws something recognisable — land,
/// water, roads and place names — without shipping a stylesheet nobody can maintain.
export function offlineStyle(background: string, ink: string, line: string): MapStyle {
  return {
    version: 8,
    glyphs: undefined,
    sources: {
      basemap: { type: "vector", url: `pmtiles://${OFFLINE_BASEMAP_URL}` },
    },
    layers: [
      { id: "backdrop", type: "background", paint: { "background-color": background } },
      {
        id: "water",
        type: "fill",
        source: "basemap",
        "source-layer": "water",
        paint: { "fill-color": line },
      },
      {
        id: "landuse",
        type: "fill",
        source: "basemap",
        "source-layer": "landuse",
        paint: { "fill-color": line, "fill-opacity": 0.35 },
      },
      {
        id: "roads",
        type: "line",
        source: "basemap",
        "source-layer": "transportation",
        paint: {
          "line-color": ink,
          "line-opacity": 0.55,
          "line-width": ["interpolate", ["linear"], ["zoom"], 6, 0.4, 14, 2.2],
        },
      },
      {
        id: "boundaries",
        type: "line",
        source: "basemap",
        "source-layer": "boundary",
        paint: { "line-color": ink, "line-opacity": 0.3, "line-dasharray": [3, 2] },
      },
    ],
  };
}

export async function fetchOnlineStyle(): Promise<MapStyle | null> {
  try {
    const response = await fetch(BASEMAP_STYLE_URL, {
      signal: AbortSignal.timeout(BASEMAP_TIMEOUT_MS),
    });
    if (!response.ok) {
      return null;
    }
    return (await response.json()) as MapStyle;
  } catch {
    return null;
  }
}

/// Whether this station has an archive to fall back to. Answered by the server rather than probed,
/// so a phone with no internet does not spend its first seconds waiting on a request that will
/// never finish.
export async function hasOfflineBasemap(): Promise<boolean> {
  try {
    const response = await fetch(OFFLINE_BASEMAP_URL, {
      method: "HEAD",
      signal: AbortSignal.timeout(BASEMAP_TIMEOUT_MS),
    });
    return response.ok;
  } catch {
    return false;
  }
}

/// Which basemap to draw, given what could be reached. Online first, the operator's archive next,
/// a plain backdrop last — and the answer says which, so the map can admit what it is showing.
export function chooseBasemap(
  online: MapStyle | null,
  offline: boolean,
  background: string,
  ink: string,
  line: string,
): { kind: BasemapKind; style: MapStyle } {
  if (online !== null) {
    return { kind: "online", style: online };
  }
  if (offline) {
    return { kind: "offline", style: offlineStyle(background, ink, line) };
  }
  return { kind: "blank", style: blankStyle(background) };
}
