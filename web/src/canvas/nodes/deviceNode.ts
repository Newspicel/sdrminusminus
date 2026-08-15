import { DIAL_ID } from "../../components/FrequencyDial";
import type { Capabilities, DeviceRef, DeviceSet, DeviceSettings } from "../../lib/types";
import { forStream } from "../../lib/useDevicePatch";
import { rxStreamCount, streamLabel } from "../graph";

export function deviceDialId(node: string, stream = 0): string {
  return stream === 0 ? `${DIAL_ID}:${node}` : `${DIAL_ID}:${node}:${stream}`;
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

export function tuneDelta(capabilities: Capabilities, stream: number, hz: number): DeviceSettings {
  return capabilities.per_stream?.tuning === true
    ? { streams: [{ stream, center_hz: hz }] }
    : { center_hz: hz };
}

export function refLabel(reference: DeviceRef): string {
  const identity = reference.key ?? reference.serial;
  return identity == null ? reference.backend : `${reference.backend} · ${identity}`;
}

export function scannerOwnsTuning(set: DeviceSet): boolean {
  return set.scanner != null && set.scanner.error == null;
}
