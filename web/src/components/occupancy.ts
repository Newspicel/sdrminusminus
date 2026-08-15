// Reading a band-occupancy report: what to show, and what a cell's colour means.
//
// The report is a flat list of frequency buckets, each with a duty cycle and a 24-slot histogram
// over the hour of the day. The panel draws it as a grid — one row per frequency, one column per
// hour — because that is the shape of the question it answers: not "how busy is this frequency"
// but "when".

import type { OccupancyBucket, OccupancyReport } from "../lib/types";

export const HOURS = 24;

/** Rows the panel draws. Past this the grid is taller than the drawer and the busiest rows —
 * which are the ones sorted to the top — scroll out of reach anyway. */
export const MAX_ROWS = 60;

export type OccupancySort = "busiest" | "frequency";

/**
 * The rows to draw, in the chosen order and capped.
 *
 * `query` filters on the printed frequency, so typing `145.5` finds the bucket that would be
 * labelled that whatever its exact centre.
 */
export function occupancyRows(
  report: OccupancyReport | null,
  sort: OccupancySort,
  query = "",
  limit = MAX_ROWS,
): OccupancyBucket[] {
  if (report === null) {
    return [];
  }
  const needle = query.trim().toLowerCase();
  const matched = report.buckets.filter(
    (bucket) => needle === "" || formatBucketHz(bucket.freq_hz).toLowerCase().includes(needle),
  );
  // The server already returns busiest-first, so that order costs nothing; frequency is a resort.
  const ordered =
    sort === "frequency" ? matched.toSorted((a, b) => a.freq_hz - b.freq_hz) : matched;
  return ordered.slice(0, limit);
}

/** `145.5000 MHz` — four decimals, which resolves a 12.5 kHz bucket without implying more. */
export function formatBucketHz(hz: number): string {
  return `${(hz / 1e6).toFixed(4)} MHz`;
}

/** `12%`, or `—` where nothing was observed. */
export function formatDuty(duty: number): string {
  if (!Number.isFinite(duty) || duty <= 0) {
    return "—";
  }
  return `${Math.round(duty * 100)}%`;
}

/**
 * Opacity for a duty cycle, on a square-root curve.
 *
 * Linear opacity makes everything below about 20% invisible, and almost every real frequency
 * lives there — a repeater in constant use is busy 5% of the day. The curve lifts the low end
 * without claiming a quiet frequency is a busy one.
 */
export function dutyAlpha(duty: number): number {
  if (!Number.isFinite(duty) || duty <= 0) {
    return 0;
  }
  return Math.min(1, Math.sqrt(Math.min(1, duty)));
}

/** The busiest hour of a bucket's day, or `null` when it was never busy. Printed beside the row
 * so the grid can be read without counting columns. */
export function busiestHour(bucket: OccupancyBucket): number | null {
  let best: number | null = null;
  let peak = 0;
  bucket.by_hour.forEach((duty, hour) => {
    if (duty > peak) {
      peak = duty;
      best = hour;
    }
  });
  return best;
}

/** `07:00`, the label for an hour column. */
export function formatHour(hour: number): string {
  return `${String(hour).padStart(2, "0")}:00`;
}

/** Whether a report has enough in it to be worth drawing a grid for. */
export function hasOccupancy(report: OccupancyReport | null): boolean {
  return report !== null && report.buckets.length > 0;
}
