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
  diversity: "Every antenna turned into step and added: about 3 dB for two",
  cancel: "The first antenna kept, what the others hear subtracted from it",
};
