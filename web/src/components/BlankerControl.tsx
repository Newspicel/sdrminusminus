import type { NoiseBlankerSettings } from "../lib/types";
import { Checkbox } from "./Checkbox";
import { AUDIO_DEFAULTS, AUDIO_LIMITS } from "./channelSettings";
import { ChipField, SettingChip } from "./face/Chips";
import { SliderField } from "./Slider";
import { useDebouncedCommit } from "./useDebouncedCommit";

export function BlankerChip({
  blanker,
  onBlanker,
}: {
  blanker: NoiseBlankerSettings;
  onBlanker: (blanker: NoiseBlankerSettings) => void;
}) {
  const threshold = blanker.threshold ?? AUDIO_DEFAULTS.blankerThreshold;
  const slider = useDebouncedCommit((next: number) => onBlanker({ ...blanker, threshold: next }));
  const shown = slider.pending ?? threshold;
  const enabled = blanker.enabled ?? false;
  return (
    <SettingChip
      label="Blanker"
      value={enabled ? `${threshold.toFixed(1)}×` : "off"}
      quiet={!enabled}
      title="Cuts impulse noise before the filter"
    >
      {() => (
        <ChipField label="Noise blanker">
          <Checkbox
            label="Noise blanker"
            checked={enabled}
            onChange={(next) => onBlanker({ ...blanker, enabled: next })}
          />
          <SliderField
            label="Noise blanker threshold"
            disabled={!enabled}
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
        </ChipField>
      )}
    </SettingChip>
  );
}
