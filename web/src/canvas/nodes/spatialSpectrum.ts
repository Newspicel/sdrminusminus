import type { Options } from "../../components/controls";
import { bearingHue } from "../../gl/spatial";
import type { SpatialSpectrumFrame } from "../../lib/frame";
import { SPATIAL_LIMITS as LIMITS, powersOfTwo } from "../../lib/limits";
import type { SpatialMethod, SpatialPeak, SpatialReading } from "../../lib/types";

export { bearingHue, columnArgmax } from "../../gl/spatial";

export type BearingFrame = "relative" | "true";
export type SpatialView = "map" | "trail";

export const PEAK_ROWS = 3;
export const BEARING_TICKS = [0, 90, 180, 270, 360] as const;

export function frameOptions(trueKnown: boolean): Options<BearingFrame> {
  return [
    { value: "relative", label: "Rel", title: "Relative to the array" },
    {
      value: "true",
      label: "True",
      title: trueKnown ? "From true north" : "Needs heading",
      disabled: !trueKnown,
    },
  ];
}

export const VIEW_OPTIONS: Options<SpatialView> = [
  { value: "map", label: "Map", title: "Bearing over frequency" },
  { value: "trail", label: "Trail", title: "Frequency over time, colour is bearing" },
];

export const METHOD_OPTIONS: Options<SpatialMethod> = [
  { value: "bartlett", label: "Bartlett", title: "Robust, wide peaks" },
  { value: "capon", label: "Capon", title: "Sharper peaks" },
  { value: "music", label: "MUSIC", title: "Sharpest, needs clean signals" },
];

const FULL_CIRCLE_DEG = 360;

export const BIN_OPTIONS: Options<number> = powersOfTwo(LIMITS.bins).map((value) => ({
  value,
  label: String(value),
}));

export function columnOptions(bins: number): Options<number> {
  return powersOfTwo(LIMITS.columns).map((value) => ({
    value,
    label: String(value),
    disabled: value > bins,
    title: value > bins ? "More than the FFT" : undefined,
  }));
}

function circleSteps(): number[] {
  const steps: number[] = [];
  for (
    let deg = Math.ceil(LIMITS.azimuth_step_deg.min);
    deg <= LIMITS.azimuth_step_deg.max;
    deg++
  ) {
    if (FULL_CIRCLE_DEG % deg === 0) {
      steps.push(deg);
    }
  }
  return steps;
}

export const STEP_OPTIONS: Options<number> = circleSteps().map((deg) => ({
  value: deg,
  label: `${deg}°`,
}));

export function binHz(col: number, bins: number, centerHz: number, spanHz: number): number {
  return centerHz - spanHz / 2 + ((col + 0.5) * spanHz) / bins;
}

export function rowDeg(row: number, bearings: number): number {
  return bearings > 0 ? (row * 360) / bearings : 0;
}

export function rowShift(offsetDeg: number, bearings: number): number {
  if (bearings <= 0) {
    return 0;
  }
  return ((Math.round((offsetDeg * bearings) / 360) % bearings) + bearings) % bearings;
}

export function rotateRows(
  frame: SpatialSpectrumFrame,
  offsetDeg: number,
  out?: Uint8Array,
): Uint8Array {
  const { bearings, bins, cells } = frame;
  const length = bearings * bins;
  const rotated = out !== undefined && out.length === length ? out : new Uint8Array(length);
  const shift = rowShift(offsetDeg, bearings);
  for (let row = 0; row < bearings; row++) {
    const source = (row - shift + bearings) % bearings;
    rotated.set(cells.subarray(source * bins, source * bins + bins), row * bins);
  }
  return rotated;
}

export function frameOffset(frame: BearingFrame, reading: SpatialReading | null): number {
  return frame === "true" ? (reading?.azimuth_deg ?? 0) : 0;
}

export function hoverAt(
  x: number,
  y: number,
  box: { w: number; h: number },
  frame: SpatialSpectrumFrame,
): { hz: number; deg: number; col: number; row: number } {
  const col = Math.min(
    frame.bins - 1,
    Math.max(0, Math.floor((x / Math.max(1, box.w)) * frame.bins)),
  );
  const row = Math.min(
    frame.bearings - 1,
    Math.max(0, Math.floor((y / Math.max(1, box.h)) * frame.bearings)),
  );
  return {
    hz: binHz(col, frame.bins, frame.centerHz, frame.spanHz),
    deg: rowDeg(row, frame.bearings),
    col,
    row,
  };
}

export function belowPeakDb(level: number, frame: SpatialSpectrumFrame): number {
  return (level / 255 - 1) * (frame.dbMax - frame.dbMin);
}

function mhz(hz: number): string {
  return `${(hz / 1e6).toFixed(3)} MHz`;
}

function degreeText(value: number): string {
  return `${Math.round(bearingHue(value)) % 360}°`;
}

export function cursorText(hz: number, bearing: number, db: number | null): string {
  const parts = [mhz(hz), degreeText(bearing)];
  if (db !== null) {
    parts.push(`${db.toFixed(0)} dB`);
  }
  return parts.join(" ");
}

export function peakBearing(peak: SpatialPeak, frame: BearingFrame, offsetDeg = 0): number {
  return frame === "true" ? (peak.true_deg ?? peak.bearing_deg + offsetDeg) : peak.bearing_deg;
}

export function peakText(peak: SpatialPeak, frame: BearingFrame, offsetDeg = 0): string {
  return `${mhz(peak.freq_hz)} ${degreeText(peakBearing(peak, frame, offsetDeg))}`;
}

export function topPeaks(reading: SpatialReading | null): SpatialPeak[] {
  return (reading?.peaks ?? []).toSorted((a, b) => b.db - a.db).slice(0, PEAK_ROWS);
}
