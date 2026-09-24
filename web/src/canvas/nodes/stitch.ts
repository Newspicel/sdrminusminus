import type { StitchMode, StitchParams } from "../../lib/types";

export const DEFAULT_STITCH_PARAMS: StitchParams = { mode: "auto", lanes: 2 };

export const STITCH_MODE_NOTE: Record<StitchMode, string> = {
  auto: "Tunes the lanes side by side into one gapless span",
  manual: "Keeps each lane where it is tuned and joins what they cover",
};
