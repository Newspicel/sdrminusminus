// Pure helpers for the scanner panel, kept out of the component so the parsing and the
// readout logic are testable without a DOM (, and the pattern the decoder views use).
import type { ChannelInfo, DeviceSet, ScannerStatus, ScanRange } from "../lib/types";

/** One line of the range editor: three MHz/kHz fields as the user typed them. */
export interface RangeInput {
  startMhz: string;
  stopMhz: string;
  stepKhz: string;
}

export const DEFAULT_RANGE: RangeInput = {
  startMhz: "145.6",
  stopMhz: "145.8",
  stepKhz: "12.5",
};

export function parseRanges(inputs: readonly RangeInput[]): { ranges: ScanRange[] } | string {
  const ranges: ScanRange[] = [];
  for (const [i, input] of inputs.entries()) {
    const start = number(input.startMhz);
    const stop = number(input.stopMhz);
    const step = number(input.stepKhz);
    const line = inputs.length > 1 ? `range ${i + 1}: ` : "";
    if (start === null || stop === null || step === null) {
      return `${line}enter numbers for start, stop and step`;
    }
    if (step <= 0) {
      return `${line}the step must be greater than zero`;
    }
    if (stop < start) {
      return `${line}the stop frequency is below the start`;
    }
    ranges.push({
      start_hz: start * 1e6,
      stop_hz: stop * 1e6,
      step_hz: step * 1e3,
    });
  }
  if (ranges.length === 0) {
    return "add at least one range";
  }
  return { ranges };
}

/** A field's value, or `null` if it is not a usable number. `Number("")` is 0, so a blank
 * field would otherwise silently scan DC. */
function number(text: string): number | null {
  if (text.trim() === "") {
    return null;
  }
  const value = Number(text);
  return Number.isFinite(value) ? value : null;
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
