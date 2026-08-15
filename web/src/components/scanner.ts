// Pure helpers for the scanner panel, kept out of the component so the parsing and the
// readout logic are testable without a DOM (and the pattern the decoder views use).
import type { ChannelInfo, DeviceSet, ScannerStatus, ScanRange } from "../lib/types";

/** One line of the range editor, in the units its fields are typed in. Numeric because the
 * editor is built from `NumberField` like every other number in the app: it holds its own draft
 * and only ever commits something finite, so "that is not a number" is not a state to model. */
export interface RangeValues {
  startMhz: number;
  stopMhz: number;
  stepKhz: number;
}

/** A row of the editor: its numbers plus an identity. Rows are removable from the middle and two
 * of them may legitimately hold the same three numbers, so position cannot key them — a removal
 * would slide the survivor's half-typed draft onto the wrong line. */
export interface RangeInput extends RangeValues {
  readonly id: string;
}

let nextRangeId = 0;

export function newRange(): RangeInput {
  nextRangeId += 1;
  return { id: `range-${nextRangeId}`, startMhz: 145.6, stopMhz: 145.8, stepKhz: 12.5 };
}

/** The smallest step the editor accepts, in kHz — a zero step expands to an infinite sweep. */
export const MIN_STEP_KHZ = 0.1;

/**
 * The wire's ranges, or the one refusal to show instead. Only the rule a single field cannot
 * enforce is left here: a stop below its start spans two fields, so no clamp can catch it.
 */
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

/** How many frequencies the ranges expand to — shown before starting, because a scan whose
 * step is a typo produces thousands of targets and the server will refuse it. */
export function targetCount(ranges: readonly ScanRange[]): number {
  return ranges.reduce(
    (total, r) => total + Math.floor((r.stop_hz - r.start_hz) / r.step_hz) + 1,
    0,
  );
}

/** The live status to render: the pushed one if this set has sent an update, else whatever the
 * last state snapshot carried. Neither alone is right — the push is fresher, but it does not
 * exist until the first update arrives, and it lingers after a stop. */
export function liveStatus(
  set: DeviceSet | null,
  pushed: ScannerStatus | undefined,
): ScannerStatus | null {
  if (!set?.scanner) {
    return null;
  }
  return pushed ?? set.scanner;
}

/** Channels a scan can park on: only the ones that actually produce something to listen to or
 * decode. Parking a scan on a channel with no output would look like a broken scan. */
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
