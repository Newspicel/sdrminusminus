import {
  allAutoTuning,
  autoTuning,
  tuneAllDelta,
  tuneDelta,
  tuningAllDelta,
  tuningDelta,
} from "../canvas/nodes/deviceNode";
import { leaveAuto } from "./autoOff";
import type { DeviceSet, Tuning } from "./types";
import { useDevicePatch } from "./useDevicePatch";

export function useRadioTune(): {
  tuneRadio: (set: DeviceSet, stream: number, hz: number) => void;
  setTuning: (set: DeviceSet, stream: number, tuning: Tuning) => void;
  tuneAll: (set: DeviceSet, hz: number) => void;
  setTuningAll: (set: DeviceSet, tuning: Tuning) => void;
} {
  const { applyPatch, cachedSettings } = useDevicePatch();
  const fresh = (set: DeviceSet): DeviceSet => ({
    ...set,
    settings: cachedSettings(set.id) ?? set.settings,
  });
  const onAuto = (set: DeviceSet, stream: number): boolean => autoTuning(fresh(set), stream);
  return {
    tuneRadio: (set, stream, hz) =>
      leaveAuto(onAuto(set, stream), () =>
        applyPatch(set.id, tuneDelta(set.capabilities, stream, hz)),
      ),
    setTuning: (set, stream, tuning) =>
      leaveAuto(tuning === "manual" && onAuto(set, stream), () =>
        applyPatch(set.id, tuningDelta(set.capabilities, stream, tuning)),
      ),
    tuneAll: (set, hz) =>
      leaveAuto(allAutoTuning(fresh(set)), () =>
        applyPatch(set.id, tuneAllDelta(set.capabilities, hz)),
      ),
    setTuningAll: (set, tuning) =>
      leaveAuto(tuning === "manual" && allAutoTuning(fresh(set)), () =>
        applyPatch(set.id, tuningAllDelta(set.capabilities, tuning)),
      ),
  };
}
