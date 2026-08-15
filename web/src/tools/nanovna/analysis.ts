import type { NanoVnaComplex, NanoVnaPoint } from "../../lib/types";
import {
  admittance,
  equivalentComponent,
  gainDb,
  groupDelays,
  impedance,
  magnitude,
  mismatchLossDb,
  phaseDeg,
  qFactor,
  returnLossDb,
  vswr,
} from "./nanovna";

export interface PointReadout {
  index: number;
  frequencyHz: number;
  s11: NanoVnaComplex;
  s21: NanoVnaComplex;
  s11Db: number;
  s11Linear: number;
  returnLossDb: number;
  s11PhaseDeg: number;
  vswr: number;
  mismatchLossDb: number;
  impedance: NanoVnaComplex | null;
  impedanceMagnitude: number;
  q: number;
  component: { kind: "capacitance" | "inductance"; value: number } | null;
  admittance: NanoVnaComplex | null;
  s21Db: number;
  s21Linear: number;
  s21PhaseDeg: number;
  insertionLossDb: number;
  groupDelayS: number;
}

export function readouts(points: readonly NanoVnaPoint[]): PointReadout[] {
  const delays = groupDelays(points);
  return points.map((point, index) => {
    const z = impedance(point.s11);
    return {
      index,
      frequencyHz: point.frequency_hz,
      s11: point.s11,
      s21: point.s21,
      s11Db: gainDb(point.s11),
      s11Linear: magnitude(point.s11),
      returnLossDb: returnLossDb(point.s11),
      s11PhaseDeg: phaseDeg(point.s11),
      vswr: vswr(point.s11),
      mismatchLossDb: mismatchLossDb(point.s11),
      impedance: z,
      impedanceMagnitude: z === null ? Number.NaN : Math.hypot(z.re, z.im),
      q: qFactor(z),
      component: z === null ? null : equivalentComponent(z.im, point.frequency_hz),
      admittance: admittance(point.s11),
      s21Db: gainDb(point.s21),
      s21Linear: magnitude(point.s21),
      s21PhaseDeg: phaseDeg(point.s21),
      insertionLossDb: -gainDb(point.s21),
      groupDelayS: delays[index] ?? Number.NaN,
    };
  });
}

export interface Band {
  startHz: number;
  stopHz: number;
  spanHz: number;
  centerHz: number;
  q: number;
  truncated: boolean;
}

export interface SweepAnalysis {
  startHz: number;
  stopHz: number;
  spanHz: number;
  count: number;
  resonance: PointReadout | null;
  vswrBands: { limit: number; band: Band | null }[];
  peak: PointReadout | null;
  transmissionBand: Band | null;
  transmitting: boolean;
}

const HALF_POWER_DB = 3;

const S21_NOISE_FLOOR_DB = -70;

const VSWR_LIMITS = [1.5, 2, 3];

export function analyse(points: readonly NanoVnaPoint[]): SweepAnalysis {
  const rows = readouts(points);
  const frequencies = rows.map((row) => row.frequencyHz);
  const first = rows[0];
  const last = rows[rows.length - 1];
  const resonance = bestBy(rows, (row) => row.vswr);
  const peak = bestBy(rows, (row) => -row.s21Db);
  const transmitting = peak !== null && peak.s21Db > S21_NOISE_FLOOR_DB;
  return {
    startHz: first?.frequencyHz ?? 0,
    stopHz: last?.frequencyHz ?? 0,
    spanHz: (last?.frequencyHz ?? 0) - (first?.frequencyHz ?? 0),
    count: rows.length,
    transmitting,
    resonance,
    vswrBands: VSWR_LIMITS.map((limit) => ({
      limit,
      band:
        resonance === null
          ? null
          : bandSpan(
              frequencies,
              rows.map((row) => row.vswr),
              resonance.index,
              limit,
              true,
            ),
    })),
    peak: transmitting ? peak : null,
    transmissionBand:
      peak === null || !transmitting
        ? null
        : bandSpan(
            frequencies,
            rows.map((row) => row.s21Db),
            peak.index,
            peak.s21Db - HALF_POWER_DB,
            false,
          ),
  };
}

function bestBy(rows: PointReadout[], score: (row: PointReadout) => number): PointReadout | null {
  let best: PointReadout | null = null;
  let bestScore = Number.POSITIVE_INFINITY;
  for (const row of rows) {
    const candidate = score(row);
    if (Number.isFinite(candidate) && candidate < bestScore) {
      best = row;
      bestScore = candidate;
    }
  }
  return best;
}

export function bandSpan(
  frequencies: readonly number[],
  values: readonly number[],
  center: number,
  threshold: number,
  below: boolean,
): Band | null {
  const inside = (index: number): boolean => {
    const value = values[index];
    if (value === undefined || !Number.isFinite(value)) {
      return false;
    }
    return below ? value <= threshold : value >= threshold;
  };
  const lastIndex = frequencies.length - 1;
  if (lastIndex < 1 || !inside(center)) {
    return null;
  }
  let low = center;
  while (low > 0 && inside(low - 1)) {
    low -= 1;
  }
  let high = center;
  while (high < lastIndex && inside(high + 1)) {
    high += 1;
  }
  const startHz =
    low === 0 ? frequencies[0] : crossing(frequencies, values, low - 1, low, threshold);
  const stopHz =
    high === lastIndex
      ? frequencies[lastIndex]
      : crossing(frequencies, values, high, high + 1, threshold);
  if (startHz === undefined || stopHz === undefined) {
    return null;
  }
  const spanHz = stopHz - startHz;
  const centerHz = (startHz + stopHz) / 2;
  return {
    startHz,
    stopHz,
    spanHz,
    centerHz,
    q: spanHz > 0 ? centerHz / spanHz : Number.POSITIVE_INFINITY,
    truncated: low === 0 || high === lastIndex,
  };
}

function crossing(
  frequencies: readonly number[],
  values: readonly number[],
  low: number,
  high: number,
  threshold: number,
): number | undefined {
  const lowHz = frequencies[low];
  const highHz = frequencies[high];
  const lowValue = values[low];
  const highValue = values[high];
  if (
    lowHz === undefined ||
    highHz === undefined ||
    lowValue === undefined ||
    highValue === undefined
  ) {
    return undefined;
  }
  if (highValue === lowValue) {
    return highHz;
  }
  return lowHz + ((threshold - lowValue) / (highValue - lowValue)) * (highHz - lowHz);
}
