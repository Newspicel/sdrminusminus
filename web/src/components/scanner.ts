import type { ChannelInfo, DeviceSet, ScannerStatus, ScanRange } from "../lib/types";

export interface RangeValues {
  startMhz: number;
  stopMhz: number;
  stepKhz: number;
}

export interface RangeInput extends RangeValues {
  readonly id: string;
}

let nextRangeId = 0;

export function newRange(): RangeInput {
  nextRangeId += 1;
  return { id: `range-${nextRangeId}`, startMhz: 145.6, stopMhz: 145.8, stepKhz: 12.5 };
}

export const MIN_STEP_KHZ = 0.1;

export function parseRanges(inputs: readonly RangeValues[]): { ranges: ScanRange[] } | string {
  const ranges: ScanRange[] = [];
  for (const [index, input] of inputs.entries()) {
    const line = inputs.length > 1 ? `range ${index + 1}: ` : "";
    if (input.stopMhz < input.startMhz) {
      return `${line}the stop frequency is below the start`;
    }
    ranges.push({
      start_hz: Math.round(input.startMhz * 1e6),
      stop_hz: Math.round(input.stopMhz * 1e6),
      step_hz: Math.round(input.stepKhz * 1e3),
    });
  }
  if (ranges.length === 0) {
    return "add at least one range";
  }
  return { ranges };
}

export function targetCount(ranges: readonly ScanRange[]): number {
  return ranges.reduce(
    (total, r) => total + Math.floor((r.stop_hz - r.start_hz) / r.step_hz) + 1,
    0,
  );
}

export function liveStatus(
  set: DeviceSet | null,
  pushed: ScannerStatus | undefined,
): ScannerStatus | null {
  if (!set?.scanner) {
    return null;
  }
  return pushed ?? set.scanner;
}

export function holdCandidates(set: DeviceSet | null): readonly ChannelInfo[] {
  return set?.channels ?? [];
}

export function scanRefusal(set: DeviceSet | null): string | null {
  return set?.capabilities.per_stream?.tuning === true
    ? "This radio tunes each receive stream independently, so a sweep has no single tuning to drive."
    : null;
}

export function formatMhz(hz: number): string {
  return `${(hz / 1e6).toFixed(4)} MHz`;
}

export function formatDb(db: number | null | undefined): string {
  return db == null || !Number.isFinite(db) ? "—" : `${db.toFixed(1)} dB`;
}
