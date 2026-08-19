import type { DfEstimate, DfStation, GuidanceMode } from "../../lib/types";

export const GUIDANCE_TEXT: Record<GuidanceMode, string> = {
  cross: "Drive across",
  approach: "Drive at it",
};

export function spreadLabel(estimate: DfEstimate | null): string {
  if (estimate === null) {
    return "—";
  }
  return `${metres(estimate.ellipse_major_m)} × ${metres(estimate.ellipse_minor_m)}`;
}

function metres(value: number): string {
  return value >= 1_000 ? `${(value / 1_000).toFixed(1)} km` : `${Math.round(value)} m`;
}

/// How long ago a station was last heard from, so a finder that has gone quiet is visibly quiet
/// rather than silently stale.
export function stationAge(station: DfStation, now: number): string {
  const seen = Date.parse(station.last_seen);
  if (Number.isNaN(seen)) {
    return "just now";
  }
  const seconds = Math.max(0, Math.round((now - seen) / 1_000));
  if (seconds < 5) {
    return "just now";
  }
  if (seconds < 90) {
    return `${seconds}s ago`;
  }
  return `${Math.round(seconds / 60)}m ago`;
}
