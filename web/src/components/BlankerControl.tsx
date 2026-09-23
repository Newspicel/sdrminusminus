import type { NoiseBlankerSettings } from "../lib/types";
import { Checkbox } from "./Checkbox";
import { AUDIO_DEFAULTS, AUDIO_LIMITS } from "./channelSettings";
import { SettingRow } from "./Settings";
import { SliderField } from "./Slider";
import { useDebouncedCommit } from "./useDebouncedCommit";

export function BlankerControl({
  blanker,
  onBlanker,
}: {
  blanker: NoiseBlankerSettings;
  onBlanker: (blanker: NoiseBlankerSettings) => void;
}) {
  const threshold = blanker.threshold ?? AUDIO_DEFAULTS.blankerThreshold;
  const slider = useDebouncedCommit((next: number) => onBlanker({ ...blanker, threshold: next }));
  const shown = slider.pending ?? threshold;
  return (
    <SettingRow label="Blanker" title="Cuts impulse noise before the filter">
      <Checkbox
        label="Noise blanker"
        checked={blanker.enabled ?? false}
        onChange={(enabled) => onBlanker({ ...blanker, enabled })}
      />
      <SliderField
        label="Noise blanker threshold"
        disabled={!(blanker.enabled ?? false)}
        min={AUDIO_LIMITS.blankerThreshold.min}
        max={AUDIO_LIMITS.blankerThreshold.max}
        step={0.5}
        value={shown}
        onChange={slider.change}
        readout={
          <>
            {shown.toFixed(1)}
            <span className="text-ink-faint">×</span>
          </>
        }
      />
    </SettingRow>
  );
}
