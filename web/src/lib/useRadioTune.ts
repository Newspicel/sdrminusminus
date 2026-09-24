import { autoTuning, tuneDelta, tuningDelta } from "../canvas/nodes/deviceNode";
import { leaveAuto } from "./autoOff";
import type { DeviceSet, Tuning } from "./types";
import { useDevicePatch } from "./useDevicePatch";

export function useRadioTune(): {
  tuneRadio: (set: DeviceSet, stream: number, hz: number) => void;
  setTuning: (set: DeviceSet, stream: number, tuning: Tuning) => void;
} {
  const { applyPatch, cachedSettings } = useDevicePatch();
  const onAuto = (set: DeviceSet, stream: number): boolean =>
    autoTuning({ ...set, settings: cachedSettings(set.id) ?? set.settings }, stream);
  return {
    tuneRadio: (set, stream, hz) =>
      leaveAuto(onAuto(set, stream), () =>
        applyPatch(set.id, tuneDelta(set.capabilities, stream, hz)),
      ),
    setTuning: (set, stream, tuning) =>
      leaveAuto(tuning === "manual" && onAuto(set, stream), () =>
        applyPatch(set.id, tuningDelta(set.capabilities, stream, tuning)),
      ),
  };
}
