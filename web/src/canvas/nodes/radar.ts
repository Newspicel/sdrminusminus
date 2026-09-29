import type { Options } from "../../components/controls";
import labels from "../../generated/labels.json";
import { GATE_TEXT } from "../../lib/arrays";
import type { RangeDopplerFrame } from "../../lib/frame";
import { LIGHT_SPEED_M_S, RADAR_LIMITS } from "../../lib/limits";
import type {
  ClutterMethod,
  PassiveRadarParams,
  ProcessorGate,
  RadarAoa,
  RadarAxes,
  RadarHealth,
  RadarProblem,
  RadarTrack,
  RadarUpdate,
} from "../../lib/types";

export const TRACK_ROWS = 6;
export const STALE_SLACK_MS = 1_000;
export const NO_TX = "no tx";
export const RADAR_TX_PORT = "tx";

type Illuminator = PassiveRadarParams["illuminator"];
type IlluminatorKind = Illuminator["kind"];
type SurveillanceSet = PassiveRadarParams["surveillance"];
type ReferenceCleaning = PassiveRadarParams["reference"];
type CfarKind = PassiveRadarParams["cfar"]["kind"];
type CfarWindow = PassiveRadarParams["cfar"]["window"];
type DopplerWindow = PassiveRadarParams["window"];
type GpuUse = PassiveRadarParams["gpu"];

export interface PlotAxes {
  rangeFirstM: number;
  rangeStepM: number;
  ranges: number;
  dopplerFirstHz: number;
  dopplerStepHz: number;
  dopplers: number;
  carrierHz: number;
  dbMin: number;
  dbMax: number;
}

export interface Box {
  w: number;
  h: number;
}

export interface SurfacePoint {
  rangeKm: number;
  dopplerHz: number;
}

export function frameAxes(frame: RangeDopplerFrame): PlotAxes {
  return {
    rangeFirstM: frame.rangeFirstM,
    rangeStepM: frame.rangeStepM,
    ranges: frame.ranges,
    dopplerFirstHz: frame.dopplerFirstHz,
    dopplerStepHz: frame.dopplerStepHz,
    dopplers: frame.dopplers,
    carrierHz: frame.carrierHz,
    dbMin: frame.dbMin,
    dbMax: frame.dbMax,
  };
}

export function readingAxes(axes: RadarAxes): PlotAxes {
  return {
    rangeFirstM: 0,
    rangeStepM: axes.range_step_m,
    ranges: axes.gates,
    dopplerFirstHz: (-(axes.doppler_rows - 1) / 2) * axes.doppler_step_hz,
    dopplerStepHz: axes.doppler_step_hz,
    dopplers: axes.doppler_rows,
    carrierHz: axes.carrier_hz,
    dbMin: RADAR_LIMITS.surface_db.min,
    dbMax: RADAR_LIMITS.surface_db.max,
  };
}

export function plotAxes(
  frame: RangeDopplerFrame | null,
  update: RadarUpdate | null,
): PlotAxes | null {
  if (frame !== null) {
    return frameAxes(frame);
  }
  if (update !== null && update.axes.gates > 0 && update.axes.doppler_rows > 0) {
    return readingAxes(update.axes);
  }
  return null;
}

export function rangeKmAt(col: number, axes: PlotAxes): number {
  return (axes.rangeFirstM + col * axes.rangeStepM) / 1_000;
}

export function dopplerHzAt(row: number, axes: PlotAxes): number {
  return axes.dopplerFirstHz + row * axes.dopplerStepHz;
}

export function velocityMps(dopplerHz: number, carrierHz: number): number | null {
  return carrierHz > 0 ? -(LIGHT_SPEED_M_S / carrierHz) * dopplerHz : null;
}

export function rangeSpanKm(axes: PlotAxes): { lo: number; hi: number } {
  const lo = axes.rangeFirstM - axes.rangeStepM / 2;
  return { lo: lo / 1_000, hi: (lo + axes.ranges * axes.rangeStepM) / 1_000 };
}

export function dopplerSpanHz(axes: PlotAxes): { lo: number; hi: number } {
  const lo = axes.dopplerFirstHz - axes.dopplerStepHz / 2;
  return { lo, hi: lo + axes.dopplers * axes.dopplerStepHz };
}

export function surfaceToPx(
  point: SurfacePoint,
  axes: PlotAxes,
  box: Box,
): { x: number; y: number } {
  const range = rangeSpanKm(axes);
  const doppler = dopplerSpanHz(axes);
  const across = range.hi - range.lo;
  const down = doppler.hi - doppler.lo;
  return {
    x: across > 0 ? ((point.rangeKm - range.lo) / across) * box.w : 0,
    y: down > 0 ? (1 - (point.dopplerHz - doppler.lo) / down) * box.h : 0,
  };
}

export function pxToSurface(x: number, y: number, axes: PlotAxes, box: Box): SurfacePoint {
  const range = rangeSpanKm(axes);
  const doppler = dopplerSpanHz(axes);
  return {
    rangeKm: range.lo + (box.w > 0 ? x / box.w : 0) * (range.hi - range.lo),
    dopplerHz: doppler.lo + (box.h > 0 ? 1 - y / box.h : 0) * (doppler.hi - doppler.lo),
  };
}

export function levelDb(level: number, axes: PlotAxes): number {
  return axes.dbMin + (level / 255) * (axes.dbMax - axes.dbMin);
}

function signed(value: number, digits: number): string {
  const text = value.toFixed(digits);
  return value > 0 && Number(text) !== 0 ? `+${text}` : text;
}

export function hoverText(point: SurfacePoint, carrierHz: number, db: number | null): string {
  const parts = [`${point.rangeKm.toFixed(1)} km`, `${signed(point.dopplerHz, 0)} Hz`];
  const speed = velocityMps(point.dopplerHz, carrierHz);
  if (speed !== null) {
    parts.push(`${signed(speed, 0)} m/s`);
  }
  if (db !== null) {
    parts.push(`${db.toFixed(0)} dB`);
  }
  return parts.join(" · ");
}

export function sortTracks(tracks: readonly RadarTrack[], limit: number): RadarTrack[] {
  return tracks.toSorted((a, b) => b.snr_db - a.snr_db || a.id - b.id).slice(0, limit);
}

export function confirmedTracks(update: RadarUpdate | null): number {
  return update?.tracks.filter((track) => track.state === "confirmed").length ?? 0;
}

export function isStale(receivedAt: number, update: RadarUpdate, now: number): boolean {
  return now - receivedAt > 3 * update.axes.hop_ms + STALE_SLACK_MS;
}

export function lostCpis(health: RadarHealth, axes: RadarAxes, inputRateHz: number | null): number {
  const rate = inputRateHz !== null && inputRateHz > 0 ? inputRateHz : axes.sample_rate_hz;
  const hopS = axes.hop_ms / 1_000;
  const samples =
    health.dropped_samples > 0 && rate > 0 && hopS > 0
      ? Math.ceil(health.dropped_samples / rate / hopS)
      : 0;
  return health.dropped_cpis + health.dropped_reports + health.lagged_updates + samples;
}

export function lostTitle(health: RadarHealth): string {
  return [
    `CPIs ${health.dropped_cpis}`,
    `discarded ${health.discarded_cpis}`,
    `reports ${health.dropped_reports}`,
    `lagged ${health.lagged_updates}`,
    `samples ${health.dropped_samples}`,
    `tracks ${health.dropped_tracks}`,
    `detections cut ${health.truncated_detections}`,
  ].join(" · ");
}

const PROBLEM_TEXT: Readonly<Record<RadarProblem["kind"], string>> = labels.radar_problem;

export function problemLabel(problem: RadarProblem): string {
  return problem.kind === "refused" ? problem.detail : PROBLEM_TEXT[problem.kind];
}

export const ILLUMINATOR_TEXT: Readonly<Record<IlluminatorKind, string>> = {
  fm: "FM",
  dab: "DAB",
  dvbt_partial: "DVB-T",
  custom: "Custom",
};

export interface Subtitle {
  text: string;
  warn: boolean;
}

export interface RadarState {
  wired: boolean;
  txWired: boolean;
  gate: ProcessorGate | null;
  update: RadarUpdate | null;
  stale: boolean;
  illuminator: IlluminatorKind;
  lanes: number;
}

export function radarSubtitle(state: RadarState): Subtitle {
  if (!state.wired) {
    return { text: "no array", warn: false };
  }
  if (state.gate !== null) {
    return { text: GATE_TEXT[state.gate].toLowerCase(), warn: true };
  }
  if (!state.txWired) {
    return { text: NO_TX, warn: true };
  }
  const problem = state.update?.problems?.[0];
  if (problem !== undefined) {
    return { text: problemLabel(problem), warn: true };
  }
  if (state.stale) {
    return { text: "stale", warn: true };
  }
  if (state.update !== null) {
    const mhz = (state.update.axes.carrier_hz / 1e6).toFixed(2);
    return { text: `${ILLUMINATOR_TEXT[state.illuminator]} ${mhz} MHz`, warn: false };
  }
  return { text: `${state.lanes} ${state.lanes === 1 ? "lane" : "lanes"}`, warn: false };
}

export function referenceText(update: RadarUpdate): string {
  const reference = update.health.reference;
  if (reference.mode === "raw") {
    return "Raw";
  }
  if (!reference.locked) {
    return "Lost";
  }
  const db = `${reference.quality_db.toFixed(0)} dB`;
  return reference.mode === "cma" ? `CMA ${db}` : `DAB ${db}`;
}

export function clutterText(suppression: readonly number[]): { value: string; title: string } {
  if (suppression.length === 0) {
    return { value: "-", title: "Direct path and clutter removed" };
  }
  const mean = suppression.reduce((sum, db) => sum + db, 0) / suppression.length;
  return {
    value: `${mean.toFixed(0)} dB`,
    title: suppression.map((db, lane) => `L${lane + 1} ${db.toFixed(0)} dB`).join(" · "),
  };
}

export function loadText(load: number): string {
  return `${Math.round(load * 100)}%`;
}

export interface BearingCell {
  text: string;
  arrayFrame: boolean;
}

export function bearingCell(aoa: RadarAoa | null | undefined): BearingCell {
  if (aoa == null) {
    return { text: "-", arrayFrame: false };
  }
  if (aoa.bearing_deg != null) {
    return { text: `${Math.round(aoa.bearing_deg)}°`, arrayFrame: false };
  }
  return { text: `${Math.round(aoa.azimuth_deg)}°`, arrayFrame: true };
}

export function adsbCell(track: RadarTrack): string {
  return track.adsb?.callsign ?? track.adsb?.icao ?? "-";
}

export function allElements(lanes: number): number[] {
  return Array.from({ length: Math.max(0, lanes) }, (_, index) => index);
}

export function surveillanceOf(set: SurveillanceSet, reference: number, lanes: number): number[] {
  return allElements(lanes).filter((element) =>
    set.kind === "all_others" ? element !== reference : (set.mask & (1 << element)) !== 0,
  );
}

export function toggledSurveillance(
  set: SurveillanceSet,
  element: number,
  reference: number,
  lanes: number,
): SurveillanceSet {
  if (element === reference) {
    return set;
  }
  const current = new Set(surveillanceOf(set, reference, lanes));
  if (current.has(element)) {
    current.delete(element);
  } else {
    current.add(element);
  }
  if (current.size === 0) {
    return set;
  }
  const others = allElements(lanes).filter((index) => index !== reference);
  if (others.every((index) => current.has(index))) {
    return { kind: "all_others" };
  }
  return { kind: "mask", mask: [...current].reduce((mask, index) => mask | (1 << index), 0) };
}

export function withReference(
  set: SurveillanceSet,
  reference: number,
): { reference_element: number; surveillance: SurveillanceSet } {
  if (set.kind === "mask" && (set.mask & (1 << reference)) !== 0) {
    const mask = set.mask & ~(1 << reference);
    return {
      reference_element: reference,
      surveillance: mask === 0 ? { kind: "all_others" } : { kind: "mask", mask },
    };
  }
  return { reference_element: reference, surveillance: set };
}

export function elementOptions(lanes: number): Options<number> {
  return allElements(Math.max(lanes, 1)).map((element) => ({
    value: element,
    label: `Element ${element + 1}`,
  }));
}

export const ILLUMINATOR_OPTIONS: Options<IlluminatorKind> = [
  { value: "fm", label: "FM" },
  { value: "dab", label: "DAB" },
  { value: "dvbt_partial", label: "DVB-T" },
  { value: "custom", label: "Custom" },
];

export function illuminatorOf(kind: IlluminatorKind, previous: Illuminator): Illuminator {
  if (kind === previous.kind) {
    return previous;
  }
  const carried = "bandwidth_hz" in previous ? previous.bandwidth_hz : null;
  switch (kind) {
    case "dvbt_partial":
      return { kind, bandwidth_hz: carried ?? RADAR_LIMITS.seed.dvbt_bandwidth_hz };
    case "custom":
      return { kind, bandwidth_hz: carried ?? RADAR_LIMITS.seed.custom_bandwidth_hz };
    default:
      return { kind };
  }
}

export const CLEANING_OPTIONS: Options<ReferenceCleaning["kind"]> = [
  { value: "off", label: "Off" },
  { value: "cma", label: "CMA", title: "Needs FM or Custom" },
  { value: "dab_remod", label: "DAB remod", title: "Needs DAB" },
];

export function cleaningFits(
  kind: ReferenceCleaning["kind"],
  illuminator: IlluminatorKind,
): boolean {
  switch (kind) {
    case "off":
      return true;
    case "cma":
      return illuminator === "fm" || illuminator === "custom";
    case "dab_remod":
      return illuminator === "dab";
  }
}

export function cleaningOptions(illuminator: IlluminatorKind): Options<ReferenceCleaning["kind"]> {
  return CLEANING_OPTIONS.map((option) => ({
    ...option,
    disabled: !cleaningFits(option.value, illuminator),
  }));
}

export function illuminatorEdit(
  kind: IlluminatorKind,
  settings: Pick<PassiveRadarParams, "illuminator" | "reference">,
): Pick<PassiveRadarParams, "illuminator" | "reference"> {
  const illuminator = illuminatorOf(kind, settings.illuminator);
  const reference = cleaningFits(settings.reference.kind, kind)
    ? settings.reference
    : { kind: "off" as const };
  return { illuminator, reference };
}

export function cleaningOf(
  kind: ReferenceCleaning["kind"],
  previous: ReferenceCleaning,
): ReferenceCleaning {
  if (kind === previous.kind) {
    return previous;
  }
  const { cma_taps: taps, cma_step: step } = RADAR_LIMITS.seed;
  return kind === "cma" ? { kind, taps, step } : { kind };
}

export const CLUTTER_OPTIONS: Options<ClutterMethod> = [
  { value: "eca_batch", label: "ECA-B" },
  { value: "eca_sliding", label: "ECA-S" },
  { value: "nlms", label: "NLMS" },
  { value: "block_nlms", label: "Block NLMS" },
  { value: "off", label: "Off" },
];

export const CFAR_OPTIONS: Options<CfarKind["kind"]> = [
  { value: "ca", label: "CA", title: "Cell averaging" },
  { value: "os", label: "OS", title: "Ordered statistic" },
  { value: "go", label: "GO", title: "Greatest of" },
];

export function cfarKindOf(kind: CfarKind["kind"], previous: CfarKind): CfarKind {
  if (kind === previous.kind) {
    return previous;
  }
  return kind === "os" ? { kind, rank: RADAR_LIMITS.seed.os_rank } : { kind };
}

export const CFAR_WINDOW_OPTIONS: Options<CfarWindow> = [
  { value: "range", label: "Range" },
  { value: "plane", label: "2D" },
];

export const PFA_OPTIONS: Options<number> = [3, 4, 5, 6, 7, 8].map((exponent) => ({
  value: 10 ** -exponent,
  label: `1e-${exponent}`,
}));

export function pfaChoice(pfa: number): number {
  const exponent = Math.min(8, Math.max(3, Math.round(-Math.log10(pfa))));
  return 10 ** -exponent;
}

export const WINDOW_OPTIONS: Options<DopplerWindow> = [
  { value: "hann", label: "Hann" },
  { value: "blackman_harris", label: "Blackman-Harris" },
  { value: "rectangular", label: "None" },
];

export const GPU_OPTIONS: Options<GpuUse> = [
  { value: "auto", label: "Auto", title: "GPU when it is faster" },
  { value: "off", label: "Off" },
];
