import type { Capabilities, DeviceRef, DeviceSet, DeviceSettings } from "../../lib/types";
import { forStream } from "../../lib/useDevicePatch";
import { rxStreamCount, streamLabel } from "../graph";

export interface TunerDial {
  stream: number;
  port: string | null;
  hz: number;
}

export function tunerDials(set: DeviceSet): TunerDial[] {
  const capabilities = set.capabilities;
  const scope = capabilities.per_stream;
  const streams = rxStreamCount(capabilities);
  if (scope?.tuning !== true || streams < 2) {
    return [{ stream: 0, port: null, hz: set.settings.center_hz ?? 0 }];
  }
  return Array.from({ length: streams }, (_, stream) => ({
    stream,
    port: streamLabel("iq", stream, streams),
    hz: forStream(set.settings, stream, scope).center_hz ?? 0,
  }));
}

export function autoTuning(set: DeviceSet): boolean {
  return (set.settings.tuning ?? "auto") === "auto";
}

export function tuneDelta(capabilities: Capabilities, stream: number, hz: number): DeviceSettings {
  return capabilities.per_stream?.tuning === true
    ? { streams: [{ stream, center_hz: hz }], tuning: "manual" }
    : { center_hz: hz, tuning: "manual" };
}

export interface Hearing {
  heard: number;
  total: number;
  tone: "ok" | "warn" | "danger";
}

export function hearing(set: DeviceSet): Hearing {
  const total = set.channels.length;
  const heard = set.channels.filter((channel) => !channel.out_of_band).length;
  const missing = set.status !== "running" || (total > 0 && heard === 0);
  return { heard, total, tone: missing ? "danger" : heard === total ? "ok" : "warn" };
}

export function refLabel(reference: DeviceRef): string {
  const identity = reference.key ?? reference.serial;
  return identity == null ? reference.backend : `${reference.backend} · ${identity}`;
}

export function scannerOwnsTuning(set: DeviceSet): boolean {
  return set.scanner != null && set.scanner.error == null;
}

const FAULTS: Record<string, string> = {
  unplugged: "is no longer attached. Plug it back in and it picks up where it left off.",
  in_use: "is open in another program. Close that one, and this radio comes back.",
  permissions:
    "may not be opened by this user. Open Check hardware for the USB permission line, which names the device node and the group that owns it.",
};

/** What a fault means for the operator, or null when only the raw message can say. */
export function faultSaid(set: DeviceSet): string | null {
  const said = set.fault == null ? undefined : FAULTS[set.fault];
  return said == null ? null : `${set.device.label} ${said}`;
}
