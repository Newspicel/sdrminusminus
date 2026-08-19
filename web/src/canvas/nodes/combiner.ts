import type { CombineMode, CombinerParams } from "../../lib/types";

export const DEFAULT_COMBINER_PARAMS: CombinerParams = {
  mode: "diversity",
  lanes: 2,
  offset_hz: 0,
  bandwidth_hz: 200_000,
  update_ms: 500,
  cal: { source: "signal", bandwidth_hz: 200_000, pilot_hz: null, track: true },
};

export const MODE_NOTE: Record<CombineMode, string> = {
  diversity:
    "Every antenna is turned into step and added, so the wanted signal reinforces and the noise does not. Two antennas are worth about 3 dB.",
  cancel:
    "The first antenna is kept and what the others hear is subtracted from it, which takes a local noise source out of a receiver you cannot move away from it. Point the others at the noise, not at the signal.",
};
