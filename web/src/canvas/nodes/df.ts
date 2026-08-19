import type { ArrayGeometry, CalState, DfParams } from "../../lib/types";

export const DF_SPECTRUM_POINTS = 360;

export const DEFAULT_DF_PARAMS: DfParams = {
  geometry: { kind: "uca", radius_m: 0.35, count: 4 },
  algorithm: "correlative",
  report_ms: 500,
  offset_hz: 0,
  bandwidth_hz: 20_000,
  sources: 1,
  beam_bearing_deg: null,
  station_id: null,
  cal: { source: "signal", bandwidth_hz: 200_000, pilot_hz: null, track: true },
};

export function elementCount(geometry: ArrayGeometry): number {
  return geometry.kind === "explicit" ? geometry.positions.length : geometry.count;
}

export function geometryOf(kind: ArrayGeometry["kind"], current: ArrayGeometry): ArrayGeometry {
  const count = elementCount(current);
  return kind === "ula"
    ? { kind: "ula", spacing_m: 0.5, count }
    : { kind: "uca", radius_m: 0.35, count };
}

export function withCount(geometry: ArrayGeometry, count: number): ArrayGeometry {
  return geometry.kind === "explicit" ? geometry : { ...geometry, count };
}

export type BeamMode = "follow" | "fixed";

export function beamMode(beamBearingDeg: number | null | undefined): BeamMode {
  return beamBearingDeg == null ? "follow" : "fixed";
}

/// Pinning the beam starts from wherever it is pointing now, so the operator holds the direction
/// the array just found instead of swinging the beam to north to begin aiming.
export function beamAzimuth(mode: BeamMode, bearingDeg: number | null | undefined): number | null {
  if (mode === "follow") {
    return null;
  }
  return ((Math.round(bearingDeg ?? 0) % 360) + 360) % 360;
}

/// Compass bearings put zero at the top and run clockwise, which is the opposite of how a screen
/// measures angles; every point on the rose goes through here so only one place knows that.
export function polarPoint(
  bearingDeg: number,
  radius: number,
  centre: number,
): { x: number; y: number } {
  const angle = ((bearingDeg - 90) * Math.PI) / 180;
  return { x: centre + radius * Math.cos(angle), y: centre + radius * Math.sin(angle) };
}

/// The closed outline of a pseudospectrum, one radius per byte.
export function spectrumPath(
  spectrum: readonly number[] | Uint8Array,
  centre: number,
  inner: number,
  outer: number,
): string {
  const points = spectrum.length;
  if (points === 0) {
    return "";
  }
  const parts: string[] = [];
  for (let index = 0; index < points; index++) {
    const level = (spectrum[index] ?? 0) / 255;
    const radius = inner + (outer - inner) * level;
    const { x, y } = polarPoint((index * 360) / points, radius, centre);
    parts.push(`${index === 0 ? "M" : "L"}${x.toFixed(2)} ${y.toFixed(2)}`);
  }
  parts.push("Z");
  return parts.join(" ");
}

export const COMPASS_MARKS = [
  { bearing: 0, label: "N" },
  { bearing: 45, label: "NE" },
  { bearing: 90, label: "E" },
  { bearing: 135, label: "SE" },
  { bearing: 180, label: "S" },
  { bearing: 225, label: "SW" },
  { bearing: 270, label: "W" },
  { bearing: 315, label: "NW" },
] as const;

export function bearingLabel(bearingDeg: number): string {
  return `${bearingDeg.toFixed(1).padStart(5, "0")}°`;
}

export type CalVerdict = "phase_unknown" | "solving" | "solved";

export function calVerdict(cal: CalState | undefined): CalVerdict {
  if (cal === undefined || cal.phase_unknown) {
    return "phase_unknown";
  }
  return cal.solved ? "solved" : "solving";
}

export const CAL_VERDICT_TEXT: Record<CalVerdict, string> = {
  phase_unknown: "phase unknown — no bearings",
  solving: "calibrating",
  solved: "calibrated",
};

export function tierLabel(cal: CalState | undefined): string {
  switch (cal?.tier) {
    case "phase_coherent":
      return "shared LO";
    case "time_sync":
      return "shared clock";
    default:
      return "not coherent";
  }
}

/// How wide a lane's quality bar is drawn, in percent.
export function laneQualityPercent(quality: number): number {
  return Math.round(Math.min(1, Math.max(0, quality)) * 100);
}
