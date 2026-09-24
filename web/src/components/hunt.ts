import { bindCarriers, bindDevices, controlledNodeOf } from "../canvas/binding";
import type { ChannelInfo, DeviceSet, HuntStatus, PatchGraph } from "../lib/types";

export const HUNT_INTERVAL_MS = 50;

export interface HuntTarget {
  set: DeviceSet;
  channel: ChannelInfo;
}

export function huntTarget(
  graph: PatchGraph,
  sets: readonly DeviceSet[],
  node: string,
): HuntTarget | null {
  const decoder = controlledNodeOf(graph, node);
  if (decoder === null) {
    return null;
  }
  const devices = bindDevices(graph, sets);
  const carrier = bindCarriers(graph, devices).get(decoder);
  const set = carrier === undefined ? undefined : devices.get(carrier.owner);
  return set === undefined || carrier === undefined ? null : { set, channel: carrier.channel };
}

export function liveHunt(
  set: DeviceSet | null,
  channel: number | null,
  pushed: HuntStatus | undefined,
): HuntStatus | null {
  const listed = set?.hunts?.find((hunt) => hunt.settings.channel === channel);
  if (listed === undefined) {
    return null;
  }
  return pushed ?? listed;
}

export function huntRefusal(target: HuntTarget | null): string | null {
  if (target === null) {
    return null;
  }
  if (target.set.scanners?.some((scanner) => scanner.settings.channel === target.channel.id)) {
    return "This decoder is scanning. Stop the scan to hunt on one frequency.";
  }
  if (target.channel.out_of_band) {
    return "The radio is tuned away from this decoder. Unlock its tuning or move it there.";
  }
  return null;
}

export function huntedHz(status: HuntStatus | null, channel: ChannelInfo | null): number | null {
  if (status !== null && (status.freq_hz ?? 0) > 0) {
    return status.freq_hz ?? null;
  }
  return channel?.settings.frequency_hz ?? null;
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
    return "-";
  }
  return `${Math.round((status.strength ?? 0) * 100)}%`;
}

export function formatHuntDb(db: number | null | undefined): string {
  return db == null || !Number.isFinite(db) ? "-" : `${db.toFixed(1)} dB`;
}
