import { bindDevices, deviceNodeOf } from "../canvas/binding";
import type { DeviceSet, HuntSettings, HuntStatus, PatchGraph } from "../lib/types";

export const DEFAULT_HUNT_SETTINGS: HuntSettings = {
  freq_hz: 433_920_000,
  bw_hz: 12_500,
  interval_ms: 50,
};

export function huntSettingsOf(graph: PatchGraph, node: string): HuntSettings {
  const found = graph.nodes.find((candidate) => candidate.id === node);
  return found?.kind === "hunt"
    ? (found.data.settings ?? DEFAULT_HUNT_SETTINGS)
    : DEFAULT_HUNT_SETTINGS;
}

export function huntDeviceSet(
  graph: PatchGraph,
  sets: readonly DeviceSet[],
  node: string,
): DeviceSet | null {
  const device = deviceNodeOf(graph, node);
  return device === null ? null : (bindDevices(graph, sets).get(device) ?? null);
}

export function liveHunt(set: DeviceSet | null, pushed: HuntStatus | undefined): HuntStatus | null {
  if (!set?.hunt) {
    return null;
  }
  return pushed ?? set.hunt;
}

export function huntRefusal(set: DeviceSet | null, freqHz: number): string | null {
  if (set === null) {
    return null;
  }
  if (set.scanner != null) {
    return "This radio is scanning. Stop the scan to hunt on one frequency.";
  }
  const ranges = set.capabilities.freq_ranges;
  if (ranges.length > 0 && !ranges.some((r) => freqHz >= r.min && freqHz <= r.max)) {
    return "That frequency is outside this radio's tuning range.";
  }
  return null;
}

/// What the operator is told about which way to walk. A hunt without a reading yet says so
/// rather than pointing them off in a direction the radio has not earned.
export type Bearing = "waiting" | "closing" | "leaving" | "steady";

export function bearing(status: HuntStatus | null): Bearing {
  if (status === null || status.readings < 2 || status.smooth_db == null) {
    return "waiting";
  }
  if (status.closing) {
    return "closing";
  }
  return (status.strength ?? 0) >= 0.9 ? "steady" : "leaving";
}

export const BEARING_LABEL: Record<Bearing, string> = {
  waiting: "listening",
  closing: "warmer",
  leaving: "colder",
  steady: "on top of it",
};

export function formatStrength(status: HuntStatus | null): string {
  if (status === null || status.readings === 0) {
    return "—";
  }
  return `${Math.round((status.strength ?? 0) * 100)}%`;
}

export function formatHuntDb(db: number | null | undefined): string {
  return db == null || !Number.isFinite(db) ? "—" : `${db.toFixed(1)} dB`;
}
