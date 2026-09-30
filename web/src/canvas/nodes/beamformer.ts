import type { Options } from "../../components/controls";
import { formatSampleRate } from "../../components/format";
import type {
  BeamformerParams,
  BeamformerReading,
  BeamMode,
  LaneWeight,
  SteerSource,
} from "../../lib/types";

export const NULL_CLOSE_DEG = 10;
export const PATTERN_FLOOR_DB = 40;

export const BEAM_MODES: Options<BeamMode> = [
  { value: "mrc", label: "MRC", title: "Best SNR from every lane" },
  { value: "das", label: "Beam", title: "Steer with equal weights" },
  { value: "mvdr", label: "MVDR", title: "Steer and suppress the rest" },
  { value: "lcmv", label: "Nulls", title: "Steer with nulls" },
  { value: "gsc", label: "GSC", title: "Steer and cancel the rest as it moves" },
  { value: "canceller", label: "Cancel", title: "Subtract what reference lanes hear" },
  { value: "cma", label: "CMA", title: "Lock onto a steady envelope" },
];

export function modeLabel(mode: BeamMode): string {
  return BEAM_MODES.find((option) => option.value === mode)?.label ?? mode;
}

export type BeamSetting =
  | "steer"
  | "nulls"
  | "auto_nulls"
  | "main"
  | "refs"
  | "taps"
  | "adaptation"
  | "step"
  | "forget"
  | "crossfade"
  | "update"
  | "carry_over"
  | "loading"
  | "noise"
  | "timeout";

const STEERED: ReadonlySet<BeamMode> = new Set(["das", "mvdr", "lcmv", "gsc"]);

function blockSettings(covariance: boolean): BeamSetting[] {
  return covariance ? ["crossfade", "update", "carry_over"] : ["crossfade", "update"];
}

function modeSettings(settings: BeamformerParams): BeamSetting[] {
  switch (settings.mode) {
    case "mrc":
      return ["noise", ...blockSettings(true)];
    case "das":
      return blockSettings(false);
    case "mvdr":
      return ["loading", ...blockSettings(true)];
    case "lcmv":
      return ["nulls", "auto_nulls", "loading", ...blockSettings(true)];
    case "gsc":
      return ["nulls", "auto_nulls", "loading", "step"];
    case "canceller":
      return ["main", "refs", "taps", ...cancellerSettings(settings)];
    case "cma":
      return ["step"];
  }
}

function cancellerSettings(settings: BeamformerParams): BeamSetting[] {
  if (settings.taps < 2) {
    return blockSettings(true);
  }
  return ["adaptation", settings.adaptation === "rls" ? "forget" : "step"];
}

export function visibleSettings(settings: BeamformerParams): ReadonlySet<BeamSetting> {
  const shown = new Set<BeamSetting>(modeSettings(settings));
  if (STEERED.has(settings.mode)) {
    shown.add("steer");
    if (settings.steer.kind === "wired") {
      shown.add("timeout");
    }
  }
  return shown;
}

export function amplitudePercent(weights: readonly LaneWeight[]): number[] {
  const linear = weights.map((weight) => 10 ** (weight.amplitude_db / 20));
  const top = Math.max(0, ...linear);
  return linear.map((value) => (top > 0 ? Math.round((value / top) * 100) : 0));
}

export function weightTitle(lane: number, weight: LaneWeight): string {
  return `L${lane + 1} ${weight.amplitude_db.toFixed(1)} dB ${Math.round(weight.phase_deg)}°`;
}

function wrapDeg(deg: number): number {
  const rounded = Math.round(deg * 10) / 10;
  return ((rounded % 360) + 360) % 360;
}

export function withNull(nulls: readonly number[], deg: number): number[] {
  if (!Number.isFinite(deg)) {
    return [...nulls];
  }
  return [...new Set([...nulls.map(wrapDeg), wrapDeg(deg)])].toSorted((a, b) => a - b);
}

export function withoutNull(nulls: readonly number[], deg: number): number[] {
  const gone = wrapDeg(deg);
  return nulls.map(wrapDeg).filter((value) => value !== gone);
}

export function nullLabel(deg: number): string {
  return `${Math.round(deg).toString().padStart(3, "0")}°`;
}

export function steerLabel(reading: BeamformerReading, steer: SteerSource): string {
  if (reading.steer_deg == null) {
    return "-";
  }
  return `${Math.round(reading.steer_deg)}° ${steer.kind === "wired" ? "DF" : "fixed"}`;
}

export function outText(reading: BeamformerReading | null): string {
  if (reading === null || !((reading.out_rate ?? 0) > 0)) {
    return "-";
  }
  const mhz = ((reading.out_center_hz ?? 0) / 1e6).toFixed(3);
  return `${mhz} MHz ${formatSampleRate(reading.out_rate ?? 0)}`;
}

export function angleGap(a: number, b: number): number {
  const gap = Math.abs(wrapDeg(a) - wrapDeg(b));
  return Math.min(gap, 360 - gap);
}

export function beamChips(reading: BeamformerReading | null): { label: string; title: string }[] {
  if (reading === null) {
    return [];
  }
  const chips: { label: string; title: string }[] = [];
  if (reading.no_steer === true) {
    chips.push({ label: "No steer", title: "No bearing to steer at" });
  }
  if (reading.steer_stale === true) {
    chips.push({ label: "Stale", title: "Last bearing is old" });
  }
  if (reading.singular === true) {
    chips.push({ label: "Singular", title: "Lanes too alike to solve" });
  }
  if ((reading.resets ?? 0) > 0 || reading.diverged === true) {
    chips.push({ label: "Reset", title: `Weights restarted ${reading.resets ?? 0} times` });
  }
  if (reading.band_full === true) {
    chips.push({ label: "Band full", title: "Band wider than the lanes" });
  }
  const steer = reading.steer_deg;
  if (steer != null && reading.nulls_deg.some((deg) => angleGap(deg, steer) < NULL_CLOSE_DEG)) {
    chips.push({ label: "Null close", title: "A null sits next to the steer" });
  }
  return chips;
}

export function patternPath(pattern: readonly number[], radius: number, centre: number): string {
  if (pattern.length === 0) {
    return "";
  }
  const points = pattern.map((level, index) => {
    const deg = (index * 360) / pattern.length;
    const reach = (Math.min(255, Math.max(0, level)) / 255) * radius;
    const radians = (deg * Math.PI) / 180;
    return `${(centre + reach * Math.sin(radians)).toFixed(1)} ${(centre - reach * Math.cos(radians)).toFixed(1)}`;
  });
  return `M${points.join("L")}Z`;
}

export function polarTick(deg: number, inner: number, outer: number, centre: number): string {
  const radians = (deg * Math.PI) / 180;
  const x = (r: number) => (centre + r * Math.sin(radians)).toFixed(1);
  const y = (r: number) => (centre - r * Math.cos(radians)).toFixed(1);
  return `M${x(inner)} ${y(inner)}L${x(outer)} ${y(outer)}`;
}

export function laneOptions(lanes: number): Options<number> {
  return Array.from({ length: Math.max(1, lanes) }, (_, lane) => ({
    value: lane,
    label: String(lane + 1),
  }));
}

export function referenceLanes(settings: BeamformerParams, lanes: number): number[] {
  const others = Array.from({ length: lanes }, (_, lane) => lane).filter(
    (lane) => lane !== settings.main_lane,
  );
  return settings.reference_lanes.length === 0
    ? others
    : settings.reference_lanes.filter((lane) => lane !== settings.main_lane);
}

export function toggledReference(
  settings: BeamformerParams,
  lane: number,
  lanes: number,
): number[] {
  const current = new Set(referenceLanes(settings, lanes));
  if (current.has(lane)) {
    current.delete(lane);
  } else {
    current.add(lane);
  }
  const others = Array.from({ length: lanes }, (_, index) => index).filter(
    (index) => index !== settings.main_lane,
  );
  if (current.size === 0) {
    return settings.reference_lanes;
  }
  return others.every((index) => current.has(index)) ? [] : [...current].toSorted((a, b) => a - b);
}
