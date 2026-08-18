import type { ChannelInfo, DeviceSet, ScannerStatus, ScanRange, ScanSession } from "../lib/types";

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

/// The other radios a scan could be spread over: running, idle of any sweep or hunt of their own,
/// and able to follow a single dial.
export function gangCandidates(
  sets: readonly DeviceSet[],
  active: DeviceSet | null,
): readonly DeviceSet[] {
  if (active === null) {
    return [];
  }
  return sets.filter(
    (set) =>
      set.id !== active.id &&
      set.status === "running" &&
      set.scanner == null &&
      set.hunt == null &&
      set.capabilities.per_stream?.tuning !== true,
  );
}

export function ganged(session: ScanSession | null, active: DeviceSet | null): readonly number[] {
  if (session === null || active === null || !session.device_sets.includes(active.id)) {
    return [];
  }
  return session.device_sets.filter((id) => id !== active.id);
}

export function sweepKind(set: DeviceSet | null, status: ScannerStatus | null): string {
  if (status !== null) {
    return status.hardware_sweep === true ? "the radio's own" : "by retuning";
  }
  return set?.capabilities.hardware_sweep === true ? "the radio's own" : "by retuning";
}

export function formatMhz(hz: number | null | undefined): string {
  return hz == null || !Number.isFinite(hz) ? "—" : `${(hz / 1e6).toFixed(4)} MHz`;
}

export function formatDb(db: number | null | undefined): string {
  return db == null || !Number.isFinite(db) ? "—" : `${db.toFixed(1)} dB`;
}
