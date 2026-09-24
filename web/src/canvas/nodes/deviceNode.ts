import { reachableHz } from "../../components/dial";
import type {
  Capabilities,
  Coherence,
  DeviceRef,
  DeviceSet,
  DeviceSettings,
  Tuning,
} from "../../lib/types";
import { forStream } from "../../lib/useDevicePatch";
import { rxStreamCount, streamLabel } from "../graph";

export function clippingSaid(set: DeviceSet): string | null {
  const lanes = set.clipping ?? [];
  if (lanes.length === 0) {
    return null;
  }
  const streams = rxStreamCount(set.capabilities);
  if (streams <= 1) {
    return "yes";
  }
  return lanes.map((lane) => streamLabel("iq", lane, streams)).join(", ");
}

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

export function lanesMerged(set: DeviceSet): boolean {
  return set.capabilities.per_stream?.tuning === true && rxStreamCount(set.capabilities) > 1;
}

export function hasLaneControls(capabilities: Capabilities): boolean {
  const scope = capabilities.per_stream;
  return (
    (scope?.gain === true && capabilities.gains.length > 0) ||
    (scope?.antenna === true && capabilities.antennas.length > 1)
  );
}

const BONDS: Record<Coherence, string | null> = {
  none: null,
  time_sync: "Shared clock",
  phase_coherent: "Phase coherent",
};

export function bondSaid(coherence: Coherence | undefined): string | null {
  return BONDS[coherence ?? "none"];
}

export function autoTuning(set: DeviceSet, stream = 0): boolean {
  const resolved = forStream(set.settings, stream, set.capabilities.per_stream);
  return (resolved.tuning ?? "auto") === "auto";
}

export function tuneDelta(capabilities: Capabilities, stream: number, hz: number): DeviceSettings {
  const center_hz = reachableHz(capabilities, hz);
  return capabilities.per_stream?.tuning === true
    ? { streams: [{ stream, center_hz, tuning: "manual" }] }
    : { center_hz, tuning: "manual" };
}

export function tuningDelta(
  capabilities: Capabilities,
  stream: number,
  tuning: Tuning,
): DeviceSettings {
  return capabilities.per_stream?.tuning === true ? { streams: [{ stream, tuning }] } : { tuning };
}

export function lockStream(locked: readonly number[], stream: number, held: boolean): number[] {
  const others = locked.filter((candidate) => candidate !== stream);
  return held ? [...others, stream].toSorted((a, b) => a - b) : others;
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

const FAULTS: Record<string, string> = {
  unplugged: "is no longer attached. Plug it back in and it picks up where it left off.",
  in_use: "is open in another program. Close that one, and this radio comes back.",
  permissions:
    "may not be opened by this user. Open Check hardware for the USB permission line, which names the device node and the group that owns it.",
};

export function refusalSaid(set: DeviceSet): string | null {
  const names = set.refused?.settings;
  if (set.error != null || names === undefined) {
    return null;
  }
  if (names.length === 0) {
    return "Radio refused the change";
  }
  const listed =
    names.length === 1 ? names[0] : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
  return `Radio refused the new ${listed}`;
}

/** What a fault means for the operator, or null when only the raw message can say. */
export function faultSaid(set: DeviceSet): string | null {
  const said = set.fault == null ? undefined : FAULTS[set.fault];
  return said == null ? null : `${set.device.label} ${said}`;
}
