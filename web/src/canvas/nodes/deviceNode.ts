// The rules a device face draws from — how many dials it gets, what a retune means, what the
// radio is called. Separate from `DeviceFace.tsx` so that file exports only components: a module
// mixing the two costs Fast Refresh the component state it would otherwise preserve.
import { DIAL_ID } from "../../components/FrequencyDial";
import type { Capabilities, DeviceRef, DeviceSet, DeviceSettings } from "../../lib/types";
import { forStream } from "../../lib/useDevicePatch";
import { rxStreamCount, streamLabel } from "../graph";

/** An id has to be unique in the document. Stream 0's dial keeps the bare id — it is the one the
 * `f` keyboard binding reaches, and a single-stream radio only has that one. */
export function deviceDialId(node: string, stream = 0): string {
  return stream === 0 ? `${DIAL_ID}:${node}` : `${DIAL_ID}:${node}:${stream}`;
}

/** One dial's worth of the face: which stream it tunes, the IQ port it answers to (`null` when
 * the radio has one tuning for every lane and the single dial needs no name), and the centre it
 * shows — the lane's own override where one exists, the radio-wide value otherwise. */
export interface TunerDial {
  stream: number;
  port: string | null;
  hz: number;
}

/**
 * The dials this radio's face draws. One, unlabelled, unless the radio itself declares tuning
 * per-stream (`Capabilities::per_stream`): a coherent array shares one tuner by definition, so
 * even four lanes get a single dial — while a radio with a synthesizer per stream gets one per
 * lane, each named after the IQ port it feeds so the dial and the wire read as the same thing.
 */
export function tunerDials(set: DeviceSet): TunerDial[] {
  const capabilities = set.capabilities;
  const scope = capabilities.per_stream;
  const streams = rxStreamCount(capabilities);
  // One dial needs no name, and two named all but the first would read as if the unnamed one were
  // the radio's rather than lane 0's.
  if (scope?.tuning !== true || streams < 2) {
    return [{ stream: 0, port: null, hz: set.settings.center_hz ?? 0 }];
  }
  return Array.from({ length: streams }, (_, stream) => ({
    stream,
    port: streamLabel("iq", stream, streams),
    hz: forStream(set.settings, stream, scope).center_hz ?? 0,
  }));
}

/** The retune delta for one dial: a stream override on a radio whose lanes tune apart — so only
 * the lane touched moves — and the radio-wide centre everywhere else. */
export function tuneDelta(capabilities: Capabilities, stream: number, hz: number): DeviceSettings {
  return capabilities.per_stream?.tuning === true
    ? { streams: [{ stream, center_hz: hz }] }
    : { center_hz: hz };
}

/** The radio a reference names, in the terms the operator would use to go and find it. A variant
 * key is shown when it narrows a serial to one operating mode. */
export function refLabel(reference: DeviceRef): string {
  const identity = reference.key ?? reference.serial;
  return identity == null ? reference.backend : `${reference.backend} · ${identity}`;
}

export function scannerOwnsTuning(set: DeviceSet): boolean {
  return set.scanner != null && set.scanner.error == null;
}
