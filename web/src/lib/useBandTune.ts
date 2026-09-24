import type { TuneTarget } from "../canvas/libraryTarget";
import { bandTuneHz } from "../components/bandPlan";
import { pushToast } from "./toasts";
import type { BandAllocation } from "./types";
import { sameMode, useTuner } from "./useTuner";

export function useBandTune(target: TuneTarget | null): (allocation: BandAllocation) => void {
  const { tune, channelType, ready } = useTuner(target);

  return (allocation) => {
    if (!ready) {
      return;
    }
    tune(bandTuneHz(allocation));
    suggestMode(allocation, channelType);
  };
}

function suggestMode(allocation: BandAllocation, channelType: string | null): void {
  const suggested = allocation.suggested;
  if (suggested == null || sameMode(suggested.type, channelType)) {
    return;
  }
  const mode = suggested.type.toUpperCase();
  pushToast(
    channelType === null
      ? `${mode} is the mode for this band: set it on a channel`
      : `${mode} is the mode for this band, not ${channelType.toUpperCase()}`,
    "info",
  );
}
