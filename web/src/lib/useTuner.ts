import { iqLanesOf } from "../canvas/binding";
import { useWorkspaceContext, type Workspace } from "../canvas/context";
import { descriptorOf, nodeOf } from "../canvas/graph";
import type { ChannelTarget, TuneTarget } from "../canvas/libraryTarget";
import { autoTuning, tuneDelta } from "../canvas/nodes/deviceNode";
import { laneOf } from "../canvas/workspaceDevice";
import { radioWindowHz, reachesHz } from "../components/channelSettings";
import type { ChannelDescriptor, DeviceSet, DeviceSettings } from "./types";
import { channelSettingsOf, useChannelEdit } from "./useChannelEdit";
import { forStream, useDevicePatch } from "./useDevicePatch";

export interface Tuner {
  tune: (hz: number) => void;
  frequencyHz: number | null;
  channelType: string | null;
  ready: boolean;
}

export function useTuner(target: TuneTarget | null): Tuner {
  const workspace = useWorkspaceContext();
  const { applyPatch } = useDevicePatch();
  const editChannel = useChannelEdit();

  const tune = (hz: number): void => {
    if (target === null || target.locked) {
      return;
    }
    if (target.kind === "device") {
      applyPatch(target.set.id, tuneDelta(target.set.capabilities, 0, hz));
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
  stream: number,
  descriptor: ChannelDescriptor | undefined,
  hz: number,
  laneCount = 1,
): DeviceSettings | null {
  if (laneCount > 1 || autoTuning(set, stream)) {
    return null;
  }
  const centerHz = forStream(set.settings, stream, set.capabilities.per_stream).center_hz ?? null;
  return reachesHz(hz, radioWindowHz(centerHz, set.settings.sample_rate, descriptor))
    ? null
    : tuneDelta(set.capabilities, stream, hz);
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
    laneOf(workspace, target.node)?.stream ?? 0,
    patch === undefined ? undefined : descriptorOf(workspace.context, patch),
    hz,
    iqLanesOf(workspace.graph, target.node).length,
  );
  if (pull !== null) {
    applyPatch(set.id, pull);
  }
}
