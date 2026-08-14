import type { StationOf } from "../decoded";
import type { ChannelParams, DecoderKind } from "../types";

/** The decoder kinds that report a position; the rest of `DecoderKind` never reaches the map. */
export const MAP_KINDS = ["adsb", "ais", "aprs"] as const satisfies readonly DecoderKind[];

export type MapKind = (typeof MAP_KINDS)[number];
export type Target = StationOf<MapKind>;

/** Semantic per-kind colours (: one accent, semantic status colours only). Chosen to
 * stay apart under deuteranopia — teal / amber / violet, not a red-green pair — and mid-tone so
 * they read on OpenFreeMap's light basemap and on the dark offline backdrop alike. */
export const KIND_STYLE: Record<MapKind, { title: string; color: string }> = {
  adsb: { title: "Aircraft", color: "#21b0b0" },
  ais: { title: "Ships", color: "#e0a458" },
  aprs: { title: "APRS", color: "#b07de0" },
};

/** A target unheard for this long is gone, not stationary: an aircraft out of range simply stops
 * transmitting. Five minutes covers an ADS-B fade and an AIS class-B vessel's three-minute
 * reporting interval without leaving ghosts on the map. */
export const TARGET_MAX_AGE_MS = 5 * 60_000;

/** How often `MapPanel` calls the store's `ageOut` to drop expired targets. Coarser than the
 * draw tick because it exists to bound memory; `targetCollection` already refuses to draw a
 * stale target in between. */
export const AGE_OUT_INTERVAL_MS = 15_000;

/** Draw tick: the map re-reads the store and calls `setData` at 2 Hz. Targets move at map scale
 * far slower than that, and it decouples redraw cost from the decoder's frame rate — ADS-B alone
 * can push hundreds of frames a second. */
export const DRAW_TICK_MS = 500;

// Minimal structural GeoJSON, assignable to the `geojson` types MapLibre's `setData` expects.
// `@types/geojson` is a transitive dependency of maplibre-gl, not one we may import directly.
export type TargetProperties = {
  id: string;
  label: string;
  /** Degrees clockwise from true north; absent when the target reports no course. */
  heading?: number;
};

export interface TargetFeature {
  type: "Feature";
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: TargetProperties;
}

export interface TargetCollection {
  type: "FeatureCollection";
  features: TargetFeature[];
}

export interface TargetDetail {
  kind: MapKind;
  id: string;
  label: string;
  freqHz: number;
  lastSeen: number;
  rows: readonly (readonly [string, string])[];
}

export function mapKindsOf(kinds: readonly string[]): MapKind[] {
  return MAP_KINDS.filter((kind) => kinds.includes(kind));
}

export function sourceId(kind: MapKind): string {
  return `targets-${kind}`;
}

export function layerId(kind: MapKind, part: "dot" | "heading" | "label"): string {
  return `targets-${kind}-${part}`;
}

export function isStale(lastSeen: number, nowMs: number, maxAgeMs = TARGET_MAX_AGE_MS): boolean {
  return lastSeen < nowMs - maxAgeMs;
}

export function targetCollection(
  stations: readonly Target[],
  nowMs: number,
  maxAgeMs = TARGET_MAX_AGE_MS,
): TargetCollection {
  const features: TargetFeature[] = [];
  for (const station of stations) {
    if (isStale(station.lastSeen, nowMs, maxAgeMs)) {
      continue;
    }
    const feature = targetFeature(station);
    if (feature !== null) {
      features.push(feature);
    }
  }
  return { type: "FeatureCollection", features };
}

/** `null` for a target that has not yet produced a position — an ADS-B identity frame arrives
 * long before a CPR pair solves, and a map has nowhere to put it. */
export function targetFeature(station: Target): TargetFeature | null {
  const coordinates = targetPosition(station);
  if (coordinates === null) {
    return null;
  }
  const properties: TargetProperties = { id: station.id, label: targetLabel(station) };
  const heading = targetHeading(station);
  if (heading !== null) {
    properties.heading = heading;
  }
  return { type: "Feature", geometry: { type: "Point", coordinates }, properties };
}

/** `[lon, lat]` — GeoJSON order, not the order every decoder reports it in — when the pair is
 * a real fix. AIS and APRS pad an unknown position with out-of-range sentinels (lat 91,
 * lon 181). */
export function geoPosition(
  lat: number | null | undefined,
  lon: number | null | undefined,
): [number, number] | null {
  if (lat == null || lon == null || !Number.isFinite(lat) || !Number.isFinite(lon)) {
    return null;
  }
  if (Math.abs(lat) > 90 || Math.abs(lon) > 180) {
    return null;
  }
  return [lon, lat];
}

export function targetPosition(station: Target): [number, number] | null {
  const { lat, lon } = station.event.data;
  return geoPosition(lat, lon);
}

/**
 * `[lon, lat]` station fixes from the wired channels' settings. Only ADS-B carries one — its
 * CPR reference is where the antenna stands — and two channels sharing an antenna produce one
 * mark, not two.
 */
export function referencePositions(params: readonly ChannelParams[]): [number, number][] {
  const seen = new Set<string>();
  const positions: [number, number][] = [];
  for (const param of params) {
    if (param.type !== "adsb") {
      continue;
    }
    const position = geoPosition(param.settings.ref_lat, param.settings.ref_lon);
    if (position === null || seen.has(position.join("/"))) {
      continue;
    }
    seen.add(position.join("/"));
    positions.push(position);
  }
  return positions;
}

export interface ReferenceCollection {
  type: "FeatureCollection";
  features: {
    type: "Feature";
    geometry: { type: "Point"; coordinates: [number, number] };
    properties: Record<string, never>;
  }[];
}

export function referenceCollection(
  positions: readonly (readonly [number, number])[],
): ReferenceCollection {
  return {
    type: "FeatureCollection",
    features: positions.map(([lon, lat]) => ({
      type: "Feature",
      geometry: { type: "Point", coordinates: [lon, lat] },
      properties: {},
    })),
  };
}

export function targetLabel(station: Target): string {
  const event = station.event;
  switch (event.kind) {
    case "adsb":
      return trimmed(event.data.callsign) ?? event.data.icao.toUpperCase();
    case "ais":
      return trimmed(event.data.name) ?? trimmed(event.data.call_sign) ?? String(event.data.mmsi);
    case "aprs":
      return event.data.source;
  }
}

export function targetHeading(station: Target): number | null {
  const event = station.event;
  switch (event.kind) {
    case "adsb":
      return bearing(event.data.track_deg);
    case "ais":
      return bearing(headingOf(event.data.heading_deg)) ?? bearing(courseOf(event.data.cog_deg));
    case "aprs":
      return bearing(event.data.course_deg);
  }
}

export function targetDetail(station: Target): TargetDetail {
  return {
    kind: station.kind,
    id: station.id,
    label: targetLabel(station),
    freqHz: station.freqHz,
    lastSeen: station.lastSeen,
    rows: [...detailRows(station), ["Frames", String(station.frames)]],
  };
}

export function formatPosition(lat: number, lon: number): string {
  return `${hemisphere(lat, "N", "S")} ${hemisphere(lon, "E", "W")}`;
}

function detailRows(station: Target): (readonly [string, string])[] {
  const position = targetPosition(station);
  const fix = position === null ? null : formatPosition(position[1], position[0]);
  const event = station.event;
  switch (event.kind) {
    case "adsb": {
      const d = event.data;
      return kept([
        ["ICAO", d.icao.toUpperCase()],
        ["Position", fix],
        ["Altitude", scalar(d.altitude_ft, 0, " ft")],
        ["Speed", scalar(d.ground_speed_kt, 0, " kt")],
        ["Track", scalar(d.track_deg, 0, "°")],
        ["V/S", scalar(d.vertical_rate_fpm, 0, " fpm")],
        ["Squawk", trimmed(d.squawk)],
        ["State", d.on_ground === true ? "on ground" : null],
      ]);
    }
    case "ais": {
      const d = event.data;
      return kept([
        ["MMSI", String(d.mmsi)],
        ["Position", fix],
        ["SOG", scalar(d.sog_kt, 1, " kt")],
        ["COG", scalar(courseOf(d.cog_deg), 0, "°")],
        ["Heading", scalar(headingOf(d.heading_deg), 0, "°")],
        ["Call sign", trimmed(d.call_sign)],
        ["Destination", trimmed(d.destination)],
      ]);
    }
    case "aprs": {
      const d = event.data;
      return kept([
        ["Source", d.source],
        ["Position", fix],
        ["Speed", scalar(d.speed_kt, 0, " kt")],
        ["Course", scalar(d.course_deg, 0, "°")],
        ["Altitude", scalar(d.altitude_ft, 0, " ft")],
        ["Message", trimmed(d.mic_e_message)],
        ["Comment", trimmed(d.comment)],
      ]);
    }
  }
}

/** Drops rows whose value is absent, so a target that reports three fields shows three rows
 * instead of a column of dashes. */
function kept(
  entries: readonly (readonly [string, string | null])[],
): (readonly [string, string])[] {
  const out: (readonly [string, string])[] = [];
  for (const [label, value] of entries) {
    if (value !== null) {
      out.push([label, value]);
    }
  }
  return out;
}

function hemisphere(deg: number, positive: string, negative: string): string {
  return `${Math.abs(deg).toFixed(4)}° ${deg < 0 ? negative : positive}`;
}

function scalar(value: number | null | undefined, digits: number, unit: string): string | null {
  return value == null || !Number.isFinite(value) ? null : `${value.toFixed(digits)}${unit}`;
}

function trimmed(value: string | null | undefined): string | null {
  const text = value?.trim() ?? "";
  return text === "" ? null : text;
}

/** ITU-R M.1371: 511 = heading not available. */
function headingOf(headingDeg: number | null | undefined): number | null {
  return headingDeg == null || headingDeg === 511 ? null : headingDeg;
}

/** ITU-R M.1371: 360.0 = course not available. */
function courseOf(cogDeg: number | null | undefined): number | null {
  return cogDeg == null || cogDeg === 360 ? null : cogDeg;
}

function bearing(deg: number | null | undefined): number | null {
  if (deg == null || !Number.isFinite(deg)) {
    return null;
  }
  return ((deg % 360) + 360) % 360;
}
