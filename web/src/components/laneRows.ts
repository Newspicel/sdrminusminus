import { rxStreamCount } from "../canvas/graph";
import type { Capabilities, DeviceSet, DeviceSettings, GainStage } from "../lib/types";
import { forStream } from "../lib/useDevicePatch";
import { agcStageIndex, isSwitch } from "./capabilities";

export interface LaneLayout {
  lanes: number;
  perLane: boolean;
  master: boolean;
  stepper: boolean;
}

export function laneLayout(caps: Capabilities): LaneLayout {
  const streams = rxStreamCount(caps);
  const perLane = caps.per_stream?.gain === true && streams > 1;
  const stepper = (caps.rx_stream_choices?.length ?? 0) > 1;
  return { lanes: perLane ? streams : 1, perLane, master: perLane || stepper, stepper };
}

export function rxStages(caps: Capabilities): GainStage[] {
  return caps.gains.filter((stage) => stage.kind !== "tx");
}

export function txStages(caps: Capabilities): GainStage[] {
  return caps.gains.filter((stage) => stage.kind === "tx");
}

export function meterStage(caps: Capabilities): GainStage | undefined {
  const stages = rxStages(caps);
  const driven = stages[agcStageIndex(stages)];
  return driven !== undefined && !isSwitch(driven) ? driven : stages.find((s) => !isSwitch(s));
}

export function laneGainDb(
  settings: DeviceSettings,
  caps: Capabilities,
  stream: number,
  stage: GainStage,
): number {
  const lane = forStream(settings, stream, caps.per_stream);
  return lane.gains?.find((gain) => gain.stage === stage.name)?.value_db ?? stage.range.min;
}

export function laneGains(set: DeviceSet, stage: GainStage): number[] {
  const { lanes } = laneLayout(set.capabilities);
  return Array.from({ length: lanes }, (_, stream) =>
    laneGainDb(set.settings, set.capabilities, stream, stage),
  );
}

export function spreadOf(values: readonly number[]): { uniform: boolean; mean: number } {
  const first = values[0] ?? 0;
  const mean = values.length === 0 ? 0 : values.reduce((a, b) => a + b, 0) / values.length;
  return { uniform: values.every((value) => value === first), mean };
}

export function allLanesGain(
  caps: Capabilities,
  stage: GainStage,
  value_db: number,
): DeviceSettings {
  const { lanes, perLane } = laneLayout(caps);
  if (!perLane) {
    return { gains: [{ stage: stage.name, value_db }] };
  }
  return {
    streams: Array.from({ length: lanes }, (_, stream) => ({
      stream,
      gains: [{ stage: stage.name, value_db }],
    })),
  };
}

export function laneGain(
  caps: Capabilities,
  stream: number,
  stage: GainStage,
  value_db: number,
): DeviceSettings {
  if (!laneLayout(caps).perLane) {
    return { gains: [{ stage: stage.name, value_db }] };
  }
  return { streams: [{ stream, gains: [{ stage: stage.name, value_db }] }] };
}

export function steppedLanes(
  choices: readonly number[],
  current: number,
  direction: number,
): number {
  const sorted = choices.toSorted((a, b) => a - b);
  const at = Math.max(0, sorted.indexOf(current));
  const next = Math.min(sorted.length - 1, Math.max(0, at + direction));
  return sorted[next] ?? current;
}
