import type { SpectrumFrame } from "../lib/frame";

export const TRACE_MODES = ["peak", "average", "min"] as const;
export type TraceMode = (typeof TRACE_MODES)[number];

const AVERAGE_FRAMES = 32;

export interface DbWindow {
  min: number;
  max: number;
}

export interface TraceState {
  peak: Float32Array;
  average: Float32Array;
  min: Float32Array;
  frames: number;
}

export function frameWindow(frame: { dbMin: number; dbMax: number }): DbWindow {
  return { min: frame.dbMin, max: frame.dbMax };
}

export function binDb(byte: number, window: DbWindow): number {
  return window.min + (byte * (window.max - window.min)) / 255;
}

export function dequantize(frame: SpectrumFrame, out: Float32Array | null): Float32Array {
  const { bins, dbMin, dbMax } = frame;
  const step = (dbMax - dbMin) / 255;
  const db = out !== null && out.length === bins.length ? out : new Float32Array(bins.length);
  for (let i = 0; i < bins.length; i++) {
    db[i] = dbMin + (bins[i] ?? 0) * step;
  }
  return db;
}

export function requantize(
  bins: Uint8Array,
  from: DbWindow,
  to: DbWindow,
  out: Uint8Array | null,
): Uint8Array {
  const dst = out !== null && out.length === bins.length ? out : new Uint8Array(bins.length);
  const span = to.max - to.min;
  if (!(span > 0)) {
    dst.fill(0);
    return dst;
  }
  const step = (from.max - from.min) / 255;
  const scale = 255 / span;
  for (let i = 0; i < bins.length; i++) {
    const db = from.min + (bins[i] ?? 0) * step;
    dst[i] = Math.min(255, Math.max(0, Math.round((db - to.min) * scale)));
  }
  return dst;
}

export function requantizeHistory(
  history: {
    rows: Uint8Array;
    count: number;
    bins: number;
    meta: readonly { dbMin: number; dbMax: number }[];
  },
  to: DbWindow,
): Uint8Array {
  const out = new Uint8Array(history.rows.length);
  for (let row = 0; row < history.count; row++) {
    const meta = history.meta[row];
    const at = row * history.bins;
    const bins = history.rows.subarray(at, at + history.bins);
    if (meta === undefined) {
      out.set(bins, at);
      continue;
    }
    out.set(requantize(bins, { min: meta.dbMin, max: meta.dbMax }, to, null), at);
  }
  return out;
}

export function newTraceState(bins: number): TraceState {
  return {
    peak: new Float32Array(bins).fill(Number.NEGATIVE_INFINITY),
    average: new Float32Array(bins),
    min: new Float32Array(bins).fill(Number.POSITIVE_INFINITY),
    frames: 0,
  };
}

export function accumulateTraces(state: TraceState | null, db: Float32Array): TraceState {
  const next = state !== null && state.peak.length === db.length ? state : newTraceState(db.length);
  const first = next.frames === 0;
  const alpha = 1 / Math.min(next.frames + 1, AVERAGE_FRAMES);
  for (let i = 0; i < db.length; i++) {
    const level = db[i] ?? Number.NEGATIVE_INFINITY;
    if (level > (next.peak[i] ?? Number.NEGATIVE_INFINITY)) {
      next.peak[i] = level;
    }
    if (level < (next.min[i] ?? Number.POSITIVE_INFINITY)) {
      next.min[i] = level;
    }
    next.average[i] = first
      ? level
      : (next.average[i] ?? level) + alpha * (level - (next.average[i] ?? level));
  }
  next.frames += 1;
  return next;
}

export function traceOf(state: TraceState, mode: TraceMode): Float32Array {
  return mode === "peak" ? state.peak : mode === "min" ? state.min : state.average;
}

export function traceUnit(db: number, window: DbWindow): number {
  const span = window.max - window.min;
  if (!(span > 0) || !Number.isFinite(db)) {
    return 0;
  }
  return Math.min(1, Math.max(0, (db - window.min) / span));
}
