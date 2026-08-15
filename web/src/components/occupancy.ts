import type { OccupancyBucket, OccupancyReport } from "../lib/types";

export const HOURS = 24;

export const MAX_ROWS = 60;

export type OccupancySort = "busiest" | "frequency";

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
  const ordered =
    sort === "frequency" ? matched.toSorted((a, b) => a.freq_hz - b.freq_hz) : matched;
  return ordered.slice(0, limit);
}

export function formatBucketHz(hz: number): string {
  return `${(hz / 1e6).toFixed(4)} MHz`;
}

export function formatDuty(duty: number): string {
  if (!Number.isFinite(duty) || duty <= 0) {
    return "—";
  }
  return `${Math.round(duty * 100)}%`;
}

export function dutyAlpha(duty: number): number {
  if (!Number.isFinite(duty) || duty <= 0) {
    return 0;
  }
  return Math.min(1, Math.sqrt(Math.min(1, duty)));
}

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

export function formatHour(hour: number): string {
  return `${String(hour).padStart(2, "0")}:00`;
}

export function hasOccupancy(report: OccupancyReport | null): boolean {
  return report !== null && report.buckets.length > 0;
}
