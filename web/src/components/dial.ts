import type { Capabilities } from "../lib/types";

const MIN_TOP_PLACE = 8;
const MAX_TOP_PLACE = 11;

export interface Range {
  min: number;
  max: number;
}

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

export function isTunable(range: Range): boolean {
  return range.max > range.min;
}

export function dialPlaces(maxHz: number): readonly number[] {
  const needed = maxHz >= 1 ? Math.floor(Math.log10(maxHz)) : 0;
  const top = Math.min(MAX_TOP_PLACE, Math.max(MIN_TOP_PLACE, needed));
  return Array.from({ length: top + 1 }, (_, i) => top - i);
}

export interface DialDigit {
  place: number;
  digit: number;
  leading: boolean;
}

export function dialDigits(hz: number, places: readonly number[]): DialDigit[] {
  const whole = Math.max(0, Math.round(hz));
  let seen = false;
  return places.map((place) => {
    const digit = Math.floor(whole / 10 ** place) % 10;
    const leading = !seen && digit === 0 && place > 6;
    seen ||= digit !== 0;
    return { place, digit, leading };
  });
}

export function stepDial(hz: number, place: number, direction: number, range: Range): number {
  return clamp(Math.round(hz) + direction * 10 ** place, range);
}

export function setDialDigit(hz: number, place: number, digit: number, range: Range): number {
  const unit = 10 ** place;
  const whole = Math.max(0, Math.round(hz));
  const current = Math.floor(whole / unit) % 10;
  return clamp(whole + (digit - current) * unit, range);
}

export function parseFrequency(text: string): number | null {
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

export function tuneTargetHz(text: string, range: Range): number | null {
  const hz = parseFrequency(text);
  return hz === null || hz < range.min || hz > range.max ? null : hz;
}

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
