import type { Options } from "../../components/controls";
import {
  RELATIVE_MARKS,
  type RoseMark,
  type RoseNeedle,
  type RoseWedge,
  TRUE_MARKS,
} from "../../components/Rose";
import { GATE_TEXT } from "../../lib/arrays";
import type {
  DfAlgorithm,
  DfParams,
  DfPeak,
  DfReading,
  ProcessorGate,
  UlaSide,
} from "../../lib/types";

export const MAX_DF_SOURCES = 15;
export const MAX_DF_PEAKS = 4;
export const NEEDS_HEADING = "Needs heading";

export interface DfAlgorithmInfo {
  value: DfAlgorithm;
  label: string;
  title: string;
  structured: boolean;
}

export const DF_ALGORITHMS: readonly DfAlgorithmInfo[] = [
  { value: "bartlett", label: "Bartlett", title: "Plain beam scan, robust", structured: false },
  { value: "capon", label: "Capon", title: "Sharper beam scan", structured: false },
  {
    value: "music",
    label: "MUSIC",
    title: "Subspace scan, splits close sources",
    structured: false,
  },
  { value: "root_music", label: "Root-MUSIC", title: "Grid-free MUSIC", structured: true },
  { value: "esprit", label: "ESPRIT", title: "Grid-free rotation fit", structured: true },
];

export const NEEDS_LINE_OR_CIRCLE = "Needs a line or circle";

export function algorithmOptions(structuredOk: boolean): Options<DfAlgorithm> {
  return DF_ALGORITHMS.map((algorithm) => {
    const refused = algorithm.structured && !structuredOk;
    return {
      value: algorithm.value,
      label: algorithm.label,
      title: refused ? NEEDS_LINE_OR_CIRCLE : algorithm.title,
      disabled: refused,
    };
  });
}

export function smoothingTitle(structuredOk: boolean): string {
  return structuredOk ? "Spatial smoothing for reflections, 0 off" : NEEDS_LINE_OR_CIRCLE;
}

export const AUTO_SOURCES = 0;
export const TOO_MANY_SOURCES = "Too many sources";

export function sourceOptions(lanes: number, stored: number | null = null): Options<number> {
  const fit = Math.max(1, Math.min(lanes - 1, MAX_DF_SOURCES));
  const counts = Array.from({ length: Math.max(fit, stored ?? 0) }, (_, index) => index + 1);
  return [
    { value: AUTO_SOURCES, label: "Auto", title: "Count sources from the signal" },
    ...counts.map((count) =>
      count > fit
        ? { value: count, label: String(count), title: TOO_MANY_SOURCES }
        : { value: count, label: String(count) },
    ),
  ];
}

export const RULE_OPTIONS: Options<DfParams["source_rule"]> = [
  { value: "dominance", label: "Dominance", title: "Count sources clearly above the rest" },
  { value: "mdl", label: "MDL", title: "Information criterion" },
];

export const PEAK_OPTIONS: Options<number> = Array.from({ length: MAX_DF_PEAKS }, (_, index) => ({
  value: index + 1,
  label: String(index + 1),
}));

export const SIDE_OPTIONS: Options<UlaSide> = [
  { value: "both", label: "Both", title: "Report front and mirror" },
  { value: "front", label: "Front", title: "Only the side ahead of the line" },
  { value: "back", label: "Back", title: "Only the side behind the line" },
];

export type BearingFrame = "relative" | "true";

export function frameOptions(trueAvailable: boolean): Options<BearingFrame> {
  return [
    { value: "relative", label: "Rel", title: "Relative to the array" },
    {
      value: "true",
      label: "True",
      title: trueAvailable ? "From true north" : NEEDS_HEADING,
      disabled: !trueAvailable,
    },
  ];
}

export function shownFrame(picked: BearingFrame | null, trueAvailable: boolean): BearingFrame {
  if (!trueAvailable) {
    return "relative";
  }
  return picked ?? "true";
}

export function roseLetters(trueFrame: boolean): readonly RoseMark[] {
  return trueFrame ? TRUE_MARKS : RELATIVE_MARKS;
}

export function frameRotation(reading: DfReading | null, frame: BearingFrame): number {
  return frame === "true" ? (reading?.azimuth_deg ?? 0) : 0;
}

export function peakDeg(peak: DfPeak, frame: BearingFrame): number | null {
  return frame === "true" ? (peak.true_deg ?? null) : peak.relative_deg;
}

function mirrorDeg(peak: DfPeak, frame: BearingFrame): number | null {
  return frame === "true" ? (peak.mirror_true_deg ?? null) : (peak.mirror_deg ?? null);
}

export function roseNeedles(peaks: readonly DfPeak[], frame: BearingFrame): RoseNeedle[] {
  const needles: RoseNeedle[] = [];
  peaks.forEach((peak, rank) => {
    const deg = peakDeg(peak, frame);
    if (deg !== null) {
      needles.push({ deg, weight: rank === 0 ? "primary" : "secondary" });
    }
    const mirror = mirrorDeg(peak, frame);
    if (mirror !== null) {
      needles.push({ deg: mirror, weight: "mirror" });
    }
  });
  return needles;
}

export function roseWedge(peaks: readonly DfPeak[], frame: BearingFrame): RoseWedge | null {
  const first = peaks[0];
  const deg = first === undefined ? null : peakDeg(first, frame);
  return first === undefined || deg === null ? null : { deg, sigmaDeg: first.sigma_deg };
}

function wrapped(deg: number): number {
  return (((Math.round(deg) % 360) + 360) % 360) % 360;
}

export function bearingLabel(deg: number): string {
  return `${String(wrapped(deg)).padStart(3, "0")}°`;
}

export function peakText(peak: DfPeak, frame: BearingFrame): string {
  const deg = peakDeg(peak, frame);
  return deg === null ? "-" : bearingLabel(deg);
}

export function sigmaText(sigmaDeg: number): string {
  return sigmaDeg < 1 ? `${sigmaDeg.toFixed(1)}°` : `${Math.round(sigmaDeg)}°`;
}

export function percentText(fraction: number): string {
  return `${Math.round(Math.min(1, Math.max(0, fraction)) * 100)}%`;
}

export function peakSummary(peak: DfPeak, frame: BearingFrame): string {
  return `${peakText(peak, frame)} ${percentText(peak.confidence)} ±${sigmaText(peak.sigma_deg)}`;
}

export interface DfChip {
  label: string;
  title: string;
  danger: boolean;
}

const READING_CHIPS: readonly {
  on: (reading: DfReading) => boolean;
  label: string;
  title: string;
}[] = [
  { on: (reading) => reading.squelched, label: "Squelch", title: "Peak below squelch" },
  {
    on: (reading) => reading.azimuth_deg == null,
    label: "No heading",
    title: "Wire a GPS with heading",
  },
  {
    on: (reading) => reading.station == null,
    label: "No position",
    title: "Wire a GPS to the array to send bearings",
  },
  {
    on: (reading) => reading.aliasing,
    label: "Aliased",
    title: "Elements more than half a wavelength apart",
  },
  {
    on: (reading) => reading.mode_aliasing === true,
    label: "Mode alias",
    title: "Too high for phase modes on this circle",
  },
  {
    on: (reading) => reading.mirror === true,
    label: "Mirror",
    title: "A line array cannot tell front from back",
  },
  {
    on: (reading) => reading.rotating === true,
    label: "Rotating",
    title: "Turning too fast, blocks skipped",
  },
  {
    on: (reading) => reading.singular === true,
    label: "Singular",
    title: "Covariance could not be inverted",
  },
  {
    on: (reading) => reading.table_out_of_range === true,
    label: "Table off",
    title: "Outside the calibration table",
  },
];

export const STALE_CHIP: DfChip = {
  label: "Stale",
  title: "No reading for three report periods",
  danger: true,
};

export function dfChips(
  reading: DfReading | null,
  gate: ProcessorGate | null,
  stale: boolean,
): DfChip[] {
  const chips: DfChip[] = [];
  if (gate !== null) {
    chips.push({ label: GATE_TEXT[gate], title: "The array holds this finder", danger: true });
  }
  if (stale) {
    chips.push(STALE_CHIP);
  }
  if (reading !== null) {
    for (const chip of READING_CHIPS) {
      if (chip.on(reading)) {
        chips.push({ label: chip.label, title: chip.title, danger: false });
      }
    }
  }
  return chips;
}

export function isStructured(algorithm: DfAlgorithm, smoothing: number): boolean {
  return (
    smoothing > 0 || DF_ALGORITHMS.some((entry) => entry.value === algorithm && entry.structured)
  );
}

export function elevationBlock(
  collinear: boolean,
  algorithm: DfAlgorithm,
  smoothing: number,
): string | null {
  if (collinear) {
    return "Needs a 2D array";
  }
  return isStructured(algorithm, smoothing) ? "Needs a grid method" : null;
}
