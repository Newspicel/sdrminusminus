import type { SpectrumHistory } from "../lib/spectrum";
import type { DbWindow } from "./spectrumTraces";

export interface FrozenRow {
  centerHz: number;
  spanHz: number;
  window: DbWindow;
  db: Float32Array;
  at: number;
}

export function frozenLength(history: SpectrumHistory): number {
  return Math.min(history.count, history.meta.length);
}

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

export function frozenCursor(index: number, length: number, visibleRows: number): number | null {
  if (length <= 0 || visibleRows <= 0) {
    return null;
  }
  const back = length - 1 - Math.min(length - 1, Math.max(0, Math.round(index)));
  return back >= visibleRows ? null : (back + 0.5) / visibleRows;
}

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
