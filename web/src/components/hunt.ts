import type {
  ChannelInfo,
  DeviceSet,
  HuntSettings,
  HuntStatus,
  HuntSweep,
  HuntSweepParams,
  SweepState,
} from "../lib/types";

export const HUNT_INTERVAL_MS = 50;

export interface HuntTarget {
  set: DeviceSet;
  channel: ChannelInfo;
}

export interface ShortText {
  label: string;
  title: string;
}

export const SCANNING: ShortText = {
  label: "Scanning",
  title: "Stop the scan to hunt one frequency",
};

export const TUNED_AWAY: ShortText = {
  label: "Tuned away",
  title: "Unlock the radio's tuning or move it here",
};

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

export function huntRefusal(target: HuntTarget | null): ShortText | null {
  if (target === null) {
    return null;
  }
  if (target.set.scanners?.some((scanner) => scanner.settings.channel === target.channel.id)) {
    return SCANNING;
  }
  if (target.channel.out_of_band) {
    return TUNED_AWAY;
  }
  return null;
}

export function huntSettings(
  channel: number,
  node: string,
  sweep: HuntSweepParams | undefined,
): HuntSettings {
  return { channel, interval_ms: HUNT_INTERVAL_MS, node, sweep };
}

export function huntedHz(status: HuntStatus | null, channel: ChannelInfo | null): number | null {
  if (status !== null && (status.freq_hz ?? 0) > 0) {
    return status.freq_hz ?? null;
  }
  return channel?.settings.frequency_hz ?? null;
}

export type Trend = "waiting" | "closing" | "leaving" | "steady";

export function trend(status: HuntStatus | null): Trend {
  if (status === null || status.readings < 2 || status.smooth_db == null) {
    return "waiting";
  }
  if (status.closing) {
    return "closing";
  }
  return (status.strength ?? 0) >= 0.9 ? "steady" : "leaving";
}

export const TREND_LABEL: Record<Trend, string> = {
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

export const SWEEP_TEXT: Record<SweepState, ShortText> = {
  off: { label: "Off", title: "Warmer and colder only" },
  idle: { label: "Ready", title: "Press Sweep and turn" },
  sweeping: { label: "Sweeping", title: "Keep turning slowly" },
  no_heading: { label: "No heading", title: "Wire a phone GPS" },
  short_span: { label: "Turn more", title: "Not enough of the circle covered yet" },
  low_contrast: { label: "Flat", title: "No clear peak, turn again" },
  poor_fit: { label: "Poor fit", title: "The levels do not match the antenna pattern" },
  heading_poor: { label: "Heading poor", title: "The compass is unsure" },
  too_fast: { label: "Too fast", title: "Turn more slowly" },
  done: { label: "Done", title: "Bearing found" },
};

export function sweepOn(sweep: HuntSweep | null | undefined): boolean {
  return sweep != null && (sweep.state ?? "off") !== "off";
}

export function degreesLabel(deg: number | null | undefined): string {
  if (deg == null || !Number.isFinite(deg)) {
    return "-";
  }
  const whole = ((Math.round(deg) % 360) + 360) % 360;
  return `${String(whole).padStart(3, "0")}°`;
}

export function coveredLabel(sweep: HuntSweep): string {
  return `${Math.round(Math.max(0, sweep.covered_deg))}°`;
}
