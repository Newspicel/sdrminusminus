// Accumulated traces and the dB window they are drawn against.
//
// Everything here works in dBFS, never in the frame's quantized bytes. The server picks each
// frame's [dbMin, dbMax] adaptively, so a byte kept from an earlier frame stands for a different
// level once a burst moves the ceiling: a trace accumulated in bytes ratchets and never recovers.

import type { SpectrumFrame } from "../lib/frame";

export const TRACE_MODES = ["peak", "average", "min"] as const;
export type TraceMode = (typeof TRACE_MODES)[number];

/** Frames the running average reaches back over. Below this many the mean is exact, so a trace
 * switched on mid-signal settles immediately rather than crawling up from the first frame. */
const AVERAGE_FRAMES = 32;

/** The dB range a plot maps onto its height. */
export interface DbWindow {
  min: number;
  max: number;
}

export interface TraceState {
  peak: Float32Array;
  average: Float32Array;
  min: Float32Array;
  /** Frames folded in since the last reset. */
  frames: number;
}

export function frameWindow(frame: { dbMin: number; dbMax: number }): DbWindow {
  return { min: frame.dbMin, max: frame.dbMax };
}

/** dB of one quantized bin under the window it was quantized against. */
export function binDb(byte: number, window: DbWindow): number {
  return window.min + (byte * (window.max - window.min)) / 255;
}

/** Expand a frame's bins to dBFS, reusing `out` when it already fits. */
export function dequantize(frame: SpectrumFrame, out: Float32Array | null): Float32Array {
  const { bins, dbMin, dbMax } = frame;
  const step = (dbMax - dbMin) / 255;
  const db = out !== null && out.length === bins.length ? out : new Float32Array(bins.length);
  for (let i = 0; i < bins.length; i++) {
    db[i] = dbMin + (bins[i] ?? 0) * step;
  }
  return db;
}

/**
 * Re-quantize a frame's bins from the window they arrived under onto `to`.
 *
 * The waterfall's texture is one byte per bin with no room for a per-row window, so a plot held
 * to a fixed dB range has to convert on the way in — otherwise the rows already in the ring are
 * coloured under a window that has since moved and the whole history means nothing.
 */
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

/**
 * Re-quantize every row a lane has kept onto one window, so the whole waterfall can be re-seeded
 * when the plot is locked to a fixed dB range or let go of one. Each row is converted from the
 * window *it* was measured under, which is the only reason the hub keeps them.
 */
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

/**
 * Fold one frame's levels into the peak, average and floor traces. Reuses `state` while the bin
 * count is unchanged, so the steady path allocates nothing; a different bin count is a different
 * frequency axis and resets all three.
 *
 * The average is a running mean of the dB values — post-detector "video" averaging, the log-domain
 * mean every receiver draws, not the mean power.
 */
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

/** Where a level sits on the plot's unit height, 0 at the window's floor and 1 at its ceiling. */
export function traceUnit(db: number, window: DbWindow): number {
  const span = window.max - window.min;
  if (!(span > 0) || !Number.isFinite(db)) {
    return 0;
  }
  return Math.min(1, Math.max(0, (db - window.min) / span));
}
