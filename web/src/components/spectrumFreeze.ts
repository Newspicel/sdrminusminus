// Reading a frozen waterfall back.
//
// The hub keeps the rows a lane has produced along with the dB window each was measured under
// (`lib/spectrum.ts`). Freezing takes a copy of that ring and scrubbing indexes into it, so the
// operator can walk back over a burst that has already scrolled past instead of trying to catch
// the next one.

import type { SpectrumHistory } from "../lib/spectrum";
import type { DbWindow } from "./spectrumTraces";

/** One row read back out of a frozen history. */
export interface FrozenRow {
  centerHz: number;
  spanHz: number;
  window: DbWindow;
  /** Levels in dBFS, one per bin. */
  db: Float32Array;
  /** Wall clock when the row arrived. */
  at: number;
}

/** How many rows a frozen history can be scrubbed over. */
export function frozenLength(history: SpectrumHistory): number {
  return Math.min(history.count, history.meta.length);
}

/**
 * Read row `index` — 0 the oldest kept, `frozenLength - 1` the newest. Out-of-range indices clamp
 * rather than returning nothing: a scrub that runs off the end should stop at the end, and the
 * plot must never blank mid-gesture.
 */
export function frozenRow(
  history: SpectrumHistory,
  index: number,
  out: Float32Array | null = null,
): FrozenRow | null {
  const length = frozenLength(history);
  if (length === 0 || history.bins === 0) {
    return null;
  }
  const at = Math.min(length - 1, Math.max(0, Math.round(index)));
  const meta = history.meta[at];
  if (meta === undefined) {
    return null;
  }
  const bins = history.rows.subarray(at * history.bins, (at + 1) * history.bins);
  const db = out !== null && out.length === bins.length ? out : new Float32Array(bins.length);
  const step = (meta.dbMax - meta.dbMin) / 255;
  for (let i = 0; i < bins.length; i++) {
    db[i] = meta.dbMin + (bins[i] ?? 0) * step;
  }
  return {
    centerHz: meta.centerHz,
    spanHz: meta.spanHz,
    window: { min: meta.dbMin, max: meta.dbMax },
    db,
    at: meta.at,
  };
}

/**
 * Where a scrubbed row sits on the waterfall, as a fraction of its height — 0 at the top edge.
 *
 * The plot draws one history row per layout pixel with the newest at the top, so a row further
 * back than the plot is tall is off the bottom and reports `null` rather than a clamped position
 * that would park the cursor on an unrelated row.
 */
export function frozenCursor(index: number, length: number, visibleRows: number): number | null {
  if (length <= 0 || visibleRows <= 0) {
    return null;
  }
  const back = length - 1 - Math.min(length - 1, Math.max(0, Math.round(index)));
  return back >= visibleRows ? null : (back + 0.5) / visibleRows;
}

/** `−1.4 s`, the age of a scrubbed row against the newest one kept. */
export function frozenAge(history: SpectrumHistory, index: number): string {
  const length = frozenLength(history);
  const newest = history.meta[length - 1];
  const row = history.meta[Math.min(length - 1, Math.max(0, Math.round(index)))];
  if (newest === undefined || row === undefined) {
    return "";
  }
  const seconds = (newest.at - row.at) / 1000;
  return seconds < 0.05 ? "live edge" : `−${seconds.toFixed(1)} s`;
}
