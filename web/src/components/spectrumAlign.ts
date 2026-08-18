import type { SpectrumHistory } from "../lib/spectrum";
import { type DbWindow, requantize, requantizeHistory } from "./spectrumTraces";

export interface FrequencyKey {
  centerHz: number;
  spanHz: number;
}

export interface FrameKey extends FrequencyKey {
  bins: number;
}

export type Retune = { kind: "none" } | { kind: "shift"; delta: number } | { kind: "reseed" };

export function retuneAction(prev: FrameKey | null, next: FrameKey): Retune {
  if (prev === null) {
    return { kind: "none" };
  }
  if (prev.centerHz === next.centerHz && prev.spanHz === next.spanHz && prev.bins === next.bins) {
    return { kind: "none" };
  }
  if (prev.spanHz === next.spanHz && prev.bins === next.bins && next.spanHz > 0) {
    return { kind: "shift", delta: (next.centerHz - prev.centerHz) / next.spanHz };
  }
  return { kind: "reseed" };
}

export function seedTarget(
  history: SpectrumHistory,
  frame: FrequencyKey | null,
): FrequencyKey | null {
  if (frame !== null && frame.spanHz > 0) {
    return { centerHz: frame.centerHz, spanHz: frame.spanHz };
  }
  const newest = history.meta[history.count - 1];
  return newest === undefined ? null : { centerHz: newest.centerHz, spanHz: newest.spanHz };
}

export function seedRows(
  history: SpectrumHistory,
  frame: FrequencyKey | null,
  held: DbWindow | null,
): Uint8Array {
  const target = seedTarget(history, frame);
  if (target !== null) {
    return alignHistory(history, target, held);
  }
  return held === null ? history.rows : requantizeHistory(history, held);
}

export function binShift(row: FrequencyKey, to: FrequencyKey, bins: number): number | null {
  if (!(to.spanHz > 0) || row.spanHz !== to.spanHz) {
    return null;
  }
  const shift = Math.round(((to.centerHz - row.centerHz) / to.spanHz) * bins);
  return Math.abs(shift) >= bins ? null : shift;
}

export function alignHistory(
  history: SpectrumHistory,
  to: FrequencyKey,
  held: DbWindow | null,
): Uint8Array {
  const { rows, count, bins, meta } = history;
  const out = new Uint8Array(count * bins);
  let scratch: Uint8Array | null = null;
  for (let row = 0; row < count; row++) {
    const at = row * bins;
    const source = rows.subarray(at, at + bins);
    const key = meta[row];
    if (key === undefined) {
      out.set(source, at);
      continue;
    }
    const shift = binShift(key, to, bins);
    if (shift === null) {
      continue;
    }
    let placed = source;
    if (held !== null) {
      scratch = requantize(source, { min: key.dbMin, max: key.dbMax }, held, scratch);
      placed = scratch;
    }
    if (shift >= 0) {
      out.set(placed.subarray(shift, bins), at);
    } else {
      out.set(placed.subarray(0, bins + shift), at - shift);
    }
  }
  return out;
}
