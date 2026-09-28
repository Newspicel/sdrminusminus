import type { Options } from "../../components/controls";
import { CAL_TEXT, failureText, SYNC_TEXT } from "../../lib/arrays";
import type {
  ArrayCalSource,
  ArrayFailure,
  ArrayLaneStatus,
  ArrayOrientation,
  ArrayRecordingStatus,
  ArrayStatus,
  CalPhase,
  Coherence,
  DeviceSet,
  HeadingSource,
  PatchGraph,
} from "../../lib/types";
import { lanesOf } from "../arrayRules";
import { ARRAY_LANE_PORT, nodeOf, portStream, rxStreamCount, streamLabel } from "../graph";
import { laneCenterHz, refLabel } from "./deviceNode";
import { ageLabel } from "./processorFace";

export const ARRAY_NAME = "Array";
export const RADIO = "Radio";
export const NO_LANES = "no lanes";
export const FAILED = "failed";

export function heldLanes(graph: PatchGraph, device: string): ReadonlyMap<number, string> {
  const held = new Map<number, string>();
  for (const edge of graph.edges ?? []) {
    const stream = edge.from.node === device ? portStream("iq", edge.from.port) : null;
    if (
      stream !== null &&
      portStream(ARRAY_LANE_PORT, edge.to.port) !== null &&
      nodeOf(graph, edge.to.node)?.kind === "array" &&
      !held.has(stream)
    ) {
      held.set(stream, edge.to.node);
    }
  }
  return held;
}

export function arrayName(graph: PatchGraph, array: string): string {
  return nodeOf(graph, array)?.label ?? ARRAY_NAME;
}

export interface LaneHold {
  array: string;
  label: string;
}

export function laneHolds(graph: PatchGraph, device: string): ReadonlyMap<number, LaneHold> {
  return new Map(
    [...heldLanes(graph, device)].map(([stream, array]) => [
      stream,
      { array, label: arrayName(graph, array) },
    ]),
  );
}

export function dialHold(
  holds: ReadonlyMap<number, LaneHold>,
  stream: number,
  merged: boolean,
): LaneHold | null {
  return (merged ? holds.get(stream) : holds.values().next().value) ?? null;
}

export interface ArrayLaneRow {
  lane: number;
  port: string;
  sourceNode: string;
  sourceLabel: string;
  status: ArrayLaneStatus | null;
}

function sourceName(
  graph: PatchGraph,
  devices: ReadonlyMap<string, DeviceSet>,
  node: string,
): string {
  const set = devices.get(node);
  if (set !== undefined) {
    return set.device.label;
  }
  const source = nodeOf(graph, node);
  if (source?.kind === "device" && source.data.device != null) {
    return refLabel(source.data.device);
  }
  return source?.label ?? RADIO;
}

export function arrayLaneRows(
  graph: PatchGraph,
  devices: ReadonlyMap<string, DeviceSet>,
  array: string,
  status: ArrayStatus | undefined,
): ArrayLaneRow[] {
  return lanesOf(graph, array).map((wire) => {
    const stream = portStream("iq", wire.source.port) ?? 0;
    const streams = rxStreamCount(devices.get(wire.source.node)?.capabilities);
    const name = sourceName(graph, devices, wire.source.node);
    return {
      lane: wire.lane + 1,
      port: wire.port,
      sourceNode: wire.source.node,
      sourceLabel: `${name} ${streamLabel("iq", stream, streams)}`,
      status: status?.lanes.find((lane) => lane.lane === wire.lane) ?? null,
    };
  });
}

export function memberDevices(graph: PatchGraph, array: string): string[] {
  return [...new Set(lanesOf(graph, array).map((wire) => wire.source.node))];
}

export const TIER_TEXT: Readonly<Record<Coherence, string>> = {
  none: "None",
  time_sync: "Shared clock",
  phase_coherent: "Shared LO",
};

export const TIER_OPTIONS: Options<Coherence> = [
  { value: "time_sync", label: TIER_TEXT.time_sync, title: "Lanes share a sample clock" },
  { value: "phase_coherent", label: TIER_TEXT.phase_coherent, title: "Lanes share clock and LO" },
];

export const PICK_ONE = "Pick one";

export function tierOptions(declared: Coherence): Options<Coherence> {
  return declared === "none"
    ? [
        { value: "none", label: PICK_ONE, title: "Say what the radios share", disabled: true },
        ...TIER_OPTIONS,
      ]
    : TIER_OPTIONS;
}

export function tierLabel(status: ArrayStatus): string {
  return status.tier_capped ? `${TIER_TEXT[status.tier]} · capped` : TIER_TEXT[status.tier];
}

const SOLVED: ReadonlySet<CalPhase> = new Set(["solved", "warm", "stale"]);

export function calLabel(status: ArrayStatus, now: number): string {
  const text = CAL_TEXT[status.cal];
  const solvedAt = status.last_solve_at == null ? Number.NaN : Date.parse(status.last_solve_at);
  return SOLVED.has(status.cal) && Number.isFinite(solvedAt)
    ? `${text} ${ageLabel(solvedAt, now)} ago`
    : text;
}

export function calTitle(status: ArrayStatus): string | undefined {
  if (status.failure != null) {
    return failureText(status.failure);
  }
  const next = status.next_check_in_s;
  return next == null ? undefined : `Next check in ${Math.max(0, Math.round(next))} s`;
}

export function syncLabel(status: ArrayStatus): string {
  const text = SYNC_TEXT[status.sync];
  return status.drift_ppm == null ? text : `${text} · ${status.drift_ppm.toFixed(1)} ppm`;
}

const DEFAULT_CAL_WIDTH_HZ = 20_000;

function widthOf(source: ArrayCalSource): number {
  return source.kind === "pilot" || source.kind === "emitter"
    ? source.bandwidth_hz
    : DEFAULT_CAL_WIDTH_HZ;
}

function offsetOf(source: ArrayCalSource): number {
  return source.kind === "pilot" || source.kind === "emitter" ? source.offset_hz : 0;
}

export function calSourceOf(kind: ArrayCalSource["kind"], from: ArrayCalSource): ArrayCalSource {
  switch (kind) {
    case "pilot":
      return { kind, offset_hz: offsetOf(from), bandwidth_hz: widthOf(from) };
    case "emitter":
      return {
        kind,
        offset_hz: offsetOf(from),
        bandwidth_hz: widthOf(from),
        bearing_deg: from.kind === "emitter" ? from.bearing_deg : 0,
      };
    default:
      return { kind };
  }
}

export function laneQualityPercent(quality: number): number {
  return Math.round(Math.min(1, Math.max(0, quality)) * 100);
}

export const CHECK_INTERVALS: Options<number> = [
  { value: 0, label: "Off" },
  { value: 10, label: "10 s" },
  { value: 60, label: "60 s" },
  { value: 300, label: "5 min" },
];

export function checkOptions(current: number): Options<number> {
  return CHECK_INTERVALS.some((option) => option.value === current)
    ? CHECK_INTERVALS
    : [...CHECK_INTERVALS, { value: current, label: `${current} s` }].toSorted(
        (a, b) => a.value - b.value,
      );
}

export const HEADING_SOURCE_TEXT: Readonly<Record<HeadingSource, string>> = {
  compass: "compass",
  course: "course",
  fused: "fused",
  gnss: "GNSS",
  sensor: "sensor",
};

function wholeDegrees(deg: number): number {
  return Math.round(((deg % 360) + 360) % 360) % 360;
}

export function degreesText(deg: number): string {
  return `${wholeDegrees(deg)}°`;
}

export function switchedOrientation(
  kind: ArrayOrientation["kind"],
  current: ArrayOrientation,
  azimuthDeg: number | null,
): ArrayOrientation {
  if (kind === current.kind) {
    return current;
  }
  return kind === "fixed"
    ? { kind, azimuth_deg: wholeDegrees(azimuthDeg ?? 0) }
    : { kind, mount_offset_deg: 0 };
}

export function headingLabel(
  orientation: ArrayOrientation,
  status: ArrayStatus | undefined,
): string {
  if (orientation.kind === "fixed") {
    return `Fixed ${degreesText(orientation.azimuth_deg)}`;
  }
  const azimuth = status?.azimuth_deg;
  if (azimuth == null) {
    return "None";
  }
  const source = status?.heading_source;
  return source == null
    ? degreesText(azimuth)
    : `${degreesText(azimuth)} ${HEADING_SOURCE_TEXT[source]}`;
}

export function arraySpanHz(
  graph: PatchGraph,
  devices: ReadonlyMap<string, DeviceSet>,
  array: string,
  status: ArrayStatus | undefined,
): number | null {
  const centres = lanesOf(graph, array).flatMap((wire) => {
    const set = devices.get(wire.source.node);
    const stream = portStream("iq", wire.source.port);
    const hz = set === undefined || stream === null ? null : laneCenterHz(set, stream);
    return hz === null ? [] : [hz];
  });
  if (centres.length === 0 || status === undefined) {
    return null;
  }
  return Math.max(...centres) - Math.min(...centres) + status.sample_rate;
}

export function laneGaps(status: ArrayStatus): number {
  return status.lanes.reduce((total, lane) => total + lane.gaps, 0);
}

export function arraySubtitle(status: ArrayStatus | undefined, wired: number): string {
  if (wired === 0) {
    return NO_LANES;
  }
  if (status?.failure != null) {
    return FAILED;
  }
  return SYNC_TEXT[status?.sync ?? "idle"].toLowerCase();
}

export function failureTitle(failure: ArrayFailure): string {
  return failure.kind === "stopped" ? failure.message : failureText(failure);
}

export function laneTitle(row: ArrayLaneRow): string {
  const lane = row.status;
  if (lane === null) {
    return `Lane ${row.lane} · ${row.sourceLabel}`;
  }
  return [
    `Lane ${row.lane}`,
    row.sourceLabel,
    `phase ${lane.phase_deg.toFixed(2)}°`,
    `gain ${lane.gain_db.toFixed(2)} dB`,
    `delay ${lane.delay_samples.toFixed(3)} smp`,
    `gaps ${lane.gaps}`,
  ].join(" · ");
}

export function recordingLabel(recording: ArrayRecordingStatus, sampleRate: number): string {
  if (recording.error != null) {
    return recording.error;
  }
  const seconds = sampleRate > 0 ? Math.floor(recording.samples / sampleRate) : 0;
  return recording.dropped > 0 ? `${seconds} s · ${recording.dropped} dropped` : `${seconds} s`;
}
