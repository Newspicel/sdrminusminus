import type { DeviceSet, ScannerStatus, ScanRange } from "../lib/types";
import { formatMhz as fixedWidthMhz } from "./format";

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
  channel: number | null,
  pushed: ScannerStatus | undefined,
): ScannerStatus | null {
  const listed = set?.scanners?.find((scanner) => scanner.settings.channel === channel);
  if (listed === undefined) {
    return null;
  }
  return pushed ?? listed;
}

export function sweepKind(set: DeviceSet | null, status: ScannerStatus | null): string {
  if (status !== null) {
    return status.hardware_sweep === true ? "the radio's own" : "by retuning";
  }
  return set?.capabilities.hardware_sweep === true ? "the radio's own" : "by retuning";
}

export function formatMhz(hz: number | null | undefined): string {
  return hz == null || !Number.isFinite(hz) ? "-" : fixedWidthMhz(hz);
}

export function formatDb(db: number | null | undefined): string {
  return db == null || !Number.isFinite(db) ? "-" : `${db.toFixed(1)} dB`;
}
