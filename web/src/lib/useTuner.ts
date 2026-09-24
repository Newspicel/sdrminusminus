import { iqLanesOf, tunedStream } from "../canvas/binding";
import { useWorkspaceContext, type Workspace } from "../canvas/context";
import { descriptorOf, nodeOf } from "../canvas/graph";
import type { ChannelTarget, TuneTarget } from "../canvas/libraryTarget";
import { autoTuning, laneCenterHz, laneRateHz, tuneDelta } from "../canvas/nodes/deviceNode";
import { laneOf } from "../canvas/workspaceDevice";
import { radioWindowHz, reachesHz } from "../components/channelSettings";
import type { ChannelDescriptor, DeviceSet, DeviceSettings } from "./types";
import { channelSettingsOf, useChannelEdit } from "./useChannelEdit";
import { forStream, useDevicePatch } from "./useDevicePatch";
import { useRadioTune } from "./useRadioTune";

export interface Tuner {
  tune: (hz: number) => void;
  frequencyHz: number | null;
  channelType: string | null;
  ready: boolean;
}

export function useTuner(target: TuneTarget | null): Tuner {
  const workspace = useWorkspaceContext();
  const { applyPatch } = useDevicePatch();
  const { tuneRadio } = useRadioTune();
  const editChannel = useChannelEdit();

  const tune = (hz: number): void => {
    if (target === null || target.locked) {
      return;
    }
    if (target.kind === "device") {
      tuneRadio(target.set, 0, hz);
      return;
    }
    editChannel(target.node, { frequency_hz: hz });
    reachFor(workspace, target, hz, applyPatch);
  };

  return {
    tune,
    frequencyHz: target === null ? null : frequencyOf(workspace, target),
    channelType:
      target === null || target.kind === "device" ? null : channelTypeOf(workspace, target.node),
    ready: target !== null && !target.locked,
  };
}

export function sameMode(mode: string | null | undefined, channelType: string | null): boolean {
  return mode == null || mode === "" || mode.toLowerCase() === channelType?.toLowerCase();
}

export function radioPullFor(
  set: DeviceSet,
  lane: { stream: number; tunes: number },
  descriptor: ChannelDescriptor | undefined,
  hz: number,
  laneCount = 1,
): DeviceSettings | null {
  if (laneCount > 1 || autoTuning(set, lane.tunes)) {
    return null;
  }
  const window = radioWindowHz(
    laneCenterHz(set, lane.stream),
    laneRateHz(set, lane.stream),
    descriptor,
  );
  return reachesHz(hz, window) ? null : tuneDelta(set.capabilities, lane.tunes, hz);
}

function tunedLane(workspace: Workspace, node: string): { stream: number; tunes: number } {
  const lane = laneOf(workspace, node);
  return lane === null
    ? { stream: 0, tunes: 0 }
    : { stream: lane.stream, tunes: tunedStream(lane) };
}

function frequencyOf(workspace: Workspace, target: TuneTarget): number | null {
  return target.kind === "device"
    ? (forStream(target.set.settings, 0, target.set.capabilities.per_stream).center_hz ?? null)
    : (channelSettingsOf(workspace, target.node)?.frequency_hz ?? null);
}

function channelTypeOf(workspace: Workspace, node: string): string | null {
  const patch = nodeOf(workspace.graph, node);
  return patch?.kind === "channel" ? patch.data.channel_type : null;
}

function reachFor(
  workspace: Workspace,
  target: ChannelTarget,
  hz: number,
  applyPatch: (ds: number, delta: DeviceSettings) => void,
): void {
  const { set } = target;
  if (set === null) {
    return;
  }
  const patch = nodeOf(workspace.graph, target.node);
  const pull = radioPullFor(
    set,
    tunedLane(workspace, target.node),
    patch === undefined ? undefined : descriptorOf(workspace.context, patch),
    hz,
    iqLanesOf(workspace.graph, target.node).length,
  );
  if (pull !== null) {
    applyPatch(set.id, pull);
  }
}
