import type { Options } from "../../components/controls";
import type { VisibilityFrame } from "../../lib/frame";
import { CORRELATOR_LIMITS as LIMITS, powersOfTwo } from "../../lib/limits";
import type { Baseline } from "../../lib/types";

export const FRINGE_POINTS = 60;
export const PHASE_SPAN = { lo: -180, hi: 180 } as const;

export interface Series {
  hz: number[];
  db: number[];
  deg: number[];
}

export function baselineLabel(baseline: Baseline): string {
  return `${baseline.a + 1}-${baseline.b + 1}`;
}

export function baselineOptions(baselines: readonly Baseline[]): Options<number> {
  return baselines.map((baseline, index) => ({ value: index, label: baselineLabel(baseline) }));
}

export function baselineText(baseline: Baseline): string {
  return [
    baseline.coherence.toFixed(2),
    `∠${Math.round(baseline.phase_deg ?? 0)}°`,
    `${baseline.delay_ns.toFixed(1)} ns`,
    `${Math.round(baseline.snr_db ?? 0)} dB`,
  ].join(" ");
}

export function binHz(bin: number, frame: VisibilityFrame): number {
  return frame.centerHz - frame.spanHz / 2 + ((bin + 0.5) * frame.spanHz) / Math.max(1, frame.bins);
}

export function baselineSeries(frame: VisibilityFrame, index: number): Series {
  const series: Series = { hz: [], db: [], deg: [] };
  if (index < 0 || index >= frame.baselines) {
    return series;
  }
  const start = index * frame.bins;
  for (let bin = 0; bin < frame.bins; bin++) {
    const amplitude = frame.amplitude[start + bin] ?? 0;
    const phase = frame.phase[start + bin] ?? 0;
    series.hz.push(binHz(bin, frame));
    series.db.push(frame.dbMin + (amplitude / 255) * (frame.dbMax - frame.dbMin));
    series.deg.push((phase / 255) * 360 - 180);
  }
  return series;
}

function scale(value: number, lo: number, hi: number, size: number): number {
  return hi > lo ? ((value - lo) / (hi - lo)) * size : 0;
}

export function linePath(
  xs: readonly number[],
  ys: readonly number[],
  box: { w: number; h: number },
  x: { lo: number; hi: number },
  y: { lo: number; hi: number },
): string {
  return xs
    .map((value, index) => {
      const px = scale(value, x.lo, x.hi, box.w).toFixed(1);
      const py = (box.h - scale(ys[index] ?? y.lo, y.lo, y.hi, box.h)).toFixed(1);
      return `${index === 0 ? "M" : "L"}${px} ${py}`;
    })
    .join("");
}

export function phasePath(
  hz: readonly number[],
  deg: readonly number[],
  box: { w: number; h: number },
  span: { lo: number; hi: number },
): string {
  let path = "";
  for (let index = 0; index < hz.length; index++) {
    const value = deg[index] ?? 0;
    const previous = deg[index - 1];
    const jump = previous === undefined || Math.abs(value - previous) > 180;
    const px = scale(hz[index] ?? 0, span.lo, span.hi, box.w).toFixed(1);
    const py = (box.h - scale(value, PHASE_SPAN.lo, PHASE_SPAN.hi, box.h)).toFixed(1);
    path += `${jump ? "M" : "L"}${px} ${py}`;
  }
  return path;
}

export function withFringe(history: readonly number[], deg: number): number[] {
  return [...history, deg].slice(-FRINGE_POINTS);
}

export function fringePath(history: readonly number[], box: { w: number; h: number }): string {
  const xs = history.map((_, index) => index);
  return phasePath(xs, history, box, { lo: 0, hi: Math.max(1, FRINGE_POINTS - 1) });
}

export const BIN_OPTIONS: Options<number> = powersOfTwo(LIMITS.bins).map((value) => ({
  value,
  label: String(value),
}));

export function channelOptions(bins: number): Options<number> {
  return powersOfTwo(LIMITS.channels).map((value) => ({
    value,
    label: String(value),
    disabled: value > bins,
    title: value > bins ? "More than the FFT" : undefined,
  }));
}
