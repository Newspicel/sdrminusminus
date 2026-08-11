// Dial arithmetic (DESIGN.md §9). All of it is integer-Hz maths on a place-value view of the
// tuned frequency, kept out of the component so the component only routes events.
//
// A "place" is a power of ten in Hz: place 6 is the megahertz digit, place 0 the hertz digit.
import type { Capabilities } from "../lib/types";

/** Below this the dial would show fewer than four MHz digits and jump width as the radio is
 * retuned; the readout must never reflow while it is being read. */
const MIN_TOP_PLACE = 8;
const MAX_TOP_PLACE = 11;

export interface Range {
  min: number;
  max: number;
}

/** The dial's limits for a receiver. Discontiguous tuners report several ranges; the dial spans
 * their envelope and the server rejects a frequency that falls in a gap — which is the honest
 * report, since only the driver knows where its holes are. */
export function tuningRange(caps: Capabilities): Range {
  const ranges = caps.freq_ranges;
  if (ranges.length === 0) {
    return { min: 0, max: 6e9 };
  }
  return {
    min: Math.min(...ranges.map((r) => r.min)),
    max: Math.max(...ranges.map((r) => r.max)),
  };
}

/** Whether the dial can move at all. A recording is pinned to the centre it was captured at,
 * and a fixed-frequency receiver reports the same shape: one range of one point. A dial that
 * turns but has every value refused is worse than a readout — and each refused retune still
 * costs a round trip and a state refresh. */
export function isTunable(range: Range): boolean {
  return range.max > range.min;
}

/** Places to render, highest first, sized to the widest frequency the device can reach. */
export function dialPlaces(maxHz: number): readonly number[] {
  const needed = maxHz >= 1 ? Math.floor(Math.log10(maxHz)) : 0;
  const top = Math.min(MAX_TOP_PLACE, Math.max(MIN_TOP_PLACE, needed));
  return Array.from({ length: top + 1 }, (_, i) => top - i);
}

export interface DialDigit {
  place: number;
  digit: number;
  /** A zero to the left of the first significant digit: drawn faint so magnitude reads before
   * any digit is parsed. */
  leading: boolean;
}

export function dialDigits(hz: number, places: readonly number[]): DialDigit[] {
  const whole = Math.max(0, Math.round(hz));
  let seen = false;
  return places.map((place) => {
    const digit = Math.floor(whole / 10 ** place) % 10;
    // The last MHz digit is never "leading": `0.100 000` must read as a frequency, not as a
    // string of faint zeros with a stray 1 in it.
    const leading = !seen && digit === 0 && place > 6;
    seen ||= digit !== 0;
    return { place, digit, leading };
  });
}

/** One unit of `place`, in `direction`. Clamped rather than wrapped: a carry out of the top
 * digit would retune the radio by a gigahertz on a stray scroll. */
export function stepDial(hz: number, place: number, direction: number, range: Range): number {
  return clamp(Math.round(hz) + direction * 10 ** place, range);
}

/** Write `digit` into `place`, leaving every other place alone. */
export function setDialDigit(hz: number, place: number, digit: number, range: Range): number {
  const unit = 10 ** place;
  const whole = Math.max(0, Math.round(hz));
  const current = Math.floor(whole / unit) % 10;
  return clamp(whole + (digit - current) * unit, range);
}

/** Free-text entry (DESIGN.md §9). A unit suffix always wins; a bare number is megahertz,
 * which is how a frequency is spoken. Returns null for anything it cannot read, so the caller
 * can leave the draft on screen rather than tuning somewhere unintended. */
export function parseFrequency(text: string): number | null {
  // Longest-first so `Hz` is read as hertz rather than as an `h` followed by a stray `z`.
  const match = /^\s*([0-9]*[.,]?[0-9]+)\s*(ghz|mhz|khz|hz|g|m|k|h)?\s*$/i.exec(text);
  if (!match) {
    return null;
  }
  const value = Number(match[1]?.replace(",", "."));
  if (!Number.isFinite(value)) {
    return null;
  }
  const unit = match[2]?.toLowerCase().charAt(0) ?? "m";
  const scale = { g: 1e9, m: 1e6, k: 1e3, h: 1 }[unit] ?? 1e6;
  return Math.round(value * scale);
}

/** The tune-step ladder the `[` and `]` keys walk. */
export const TUNE_STEPS_HZ = [10, 100, 1_000, 5_000, 12_500, 25_000, 100_000, 1_000_000] as const;

export function formatStep(hz: number): string {
  if (hz >= 1e6) {
    return `${hz / 1e6} MHz`;
  }
  if (hz >= 1e3) {
    return `${trimZeros(hz / 1e3)} kHz`;
  }
  return `${hz} Hz`;
}

function trimZeros(value: number): string {
  return String(Number(value.toFixed(3)));
}

function clamp(hz: number, range: Range): number {
  return Math.min(range.max, Math.max(range.min, hz));
}
