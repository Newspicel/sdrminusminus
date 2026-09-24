import { useWorkspaceContext } from "../canvas/context";
import { followedDecoder } from "../canvas/nodes/autoFollow";
import { autoTuning, laneCenterHz, tuneDelta, tuningDelta } from "../canvas/nodes/deviceNode";
import { pushNote } from "./toasts";
import type { DeviceSet } from "./types";
import { channelSettingsOf, useChannelEdit } from "./useChannelEdit";
import { useDevicePatch } from "./useDevicePatch";

export interface RadioTarget {
  node: string;
  set: DeviceSet;
  stream: number;
  tunes: number;
}

export interface RadioTune {
  followed: string | null;
  hz: number | null;
  tune: (hz: number) => void;
}

const MANUAL_NOTE = "Manual tuning: the radio stays put";

type DevicePatch = ReturnType<typeof useDevicePatch>;

export function takeOver(
  applyPatch: DevicePatch["applyPatch"],
  cachedSettings: DevicePatch["cachedSettings"],
  { set, tunes }: Pick<RadioTarget, "set" | "tunes">,
  hz: number,
): void {
  const settings = cachedSettings(set.id) ?? set.settings;
  const wasAuto = autoTuning({ ...set, settings }, tunes);
  applyPatch(set.id, tuneDelta(set.capabilities, tunes, hz));
  if (wasAuto) {
    pushNote(MANUAL_NOTE, {
      label: "Back to Auto",
      run: () => applyPatch(set.id, tuningDelta(set.capabilities, tunes, "auto")),
    });
  }
}

export function useRadioTune(target: RadioTarget | null): RadioTune {
  const workspace = useWorkspaceContext();
  const { applyPatch, cachedSettings } = useDevicePatch();
  const editChannel = useChannelEdit();
  const followed =
    target !== null && autoTuning(target.set, target.tunes)
      ? followedDecoder(workspace, target.node, target.stream)
      : null;

  const tune = (hz: number): void => {
    if (target === null) {
      return;
    }
    if (followed !== null) {
      editChannel(followed, { frequency_hz: Math.round(hz) });
      return;
    }
    takeOver(applyPatch, cachedSettings, target, hz);
  };

  const hz =
    target === null
      ? null
      : followed === null
        ? laneCenterHz(target.set, target.stream)
        : (channelSettingsOf(workspace, followed)?.frequency_hz ?? null);

  return { followed, hz, tune };
}
