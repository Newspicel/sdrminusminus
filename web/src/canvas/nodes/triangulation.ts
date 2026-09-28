import type { DfEstimate, DfStation, NavTargetKind } from "../../lib/types";

export const NAV_TEXT: Record<NavTargetKind, string> = {
  probe: "Drive across",
  estimate: "Drive at it",
};

export function spreadLabel(estimate: DfEstimate | null): string {
  if (estimate === null) {
    return "-";
  }
  return `${metres(estimate.ellipse_major_m)} × ${metres(estimate.ellipse_minor_m)}`;
}

function metres(value: number): string {
  return value >= 1_000 ? `${(value / 1_000).toFixed(1)} km` : `${Math.round(value)} m`;
}

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
