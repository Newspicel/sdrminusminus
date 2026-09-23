import { Plus, X } from "lucide-react";
import type { AudioAgcMode, AudioProcessing, DenoiseMode, NotchSettings } from "../lib/types";
import { Button } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import {
  AUDIO_DEFAULTS,
  AUDIO_LIMITS,
  audioChainActive,
  mergeAudio,
  withNotchAdded,
  withNotchAt,
  withNotchRemoved,
} from "./channelSettings";
import { BTN_SM, ICON_BTN_SM, type Options } from "./controls";
import { Icon } from "./Icon";
import { NumberField } from "./NumberField";
import { Segmented } from "./Segmented";
import { SettingGroup, SettingRow } from "./Settings";
import { SliderField } from "./Slider";
import { useDebouncedCommit } from "./useDebouncedCommit";

const AGC_MODES: Options<AudioAgcMode> = [
  { value: "off", label: "Off" },
  { value: "slow", label: "Slow" },
  { value: "medium", label: "Med" },
  { value: "fast", label: "Fast" },
];

const DENOISE_MODES: Options<DenoiseMode> = [
  {
    value: "spectral",
    label: "Spectral",
    title: "Light and fast, for steady hiss",
  },
  { value: "neural", label: "Neural", title: "DPDFNet speech model, for voice" },
];

export function AudioControls({
  audio,
  onAudio,
}: {
  audio: AudioProcessing;
  onAudio: (audio: AudioProcessing) => void;
}) {
  const notches = audio.notches ?? [];
  const edit = (patch: Partial<AudioProcessing>) => onAudio(mergeAudio(audio, patch));

  const clicks = audio.click_removal ?? {};
  const clickThreshold = clicks.threshold ?? AUDIO_DEFAULTS.clickThreshold;
  const clickSlider = useDebouncedCommit((threshold: number) =>
    edit({ click_removal: { ...clicks, threshold } }),
  );

  const denoise = audio.denoise ?? {};
  const denoiseStrength = denoise.strength ?? AUDIO_DEFAULTS.denoiseStrength;
  const denoiseSlider = useDebouncedCommit((strength: number) =>
    edit({ denoise: { ...denoise, strength } }),
  );

  const filter = audio.filter ?? {};
  const lowHz = filter.low_hz ?? AUDIO_DEFAULTS.filterLowHz;
  const highHz = filter.high_hz ?? AUDIO_DEFAULTS.filterHighHz;

  return (
    <>
      <SettingGroup
        label={
          <>
            Audio
            {audioChainActive(audio) && <span className="text-accent"> on</span>}
          </>
        }
        action={
          <Button
            type="button"
            className={BTN_SM}
            disabled={notches.length >= AUDIO_LIMITS.maxNotches}
            title={`Up to ${AUDIO_LIMITS.maxNotches} notches`}
            onClick={() => {
              const next = withNotchAdded(notches);
              if (next !== null) {
                edit({ notches: next });
              }
            }}
          >
            <Icon glyph={Plus} size={12} />
            notch
          </Button>
        }
      >
        <SettingRow label="AGC">
          <Segmented
            label="Audio AGC speed"
            value={audio.agc ?? "off"}
            options={AGC_MODES}
            onChange={(agc) => edit({ agc })}
          />
        </SettingRow>

        <SettingRow label="De-click">
          <Checkbox
            label="Click removal"
            checked={clicks.enabled ?? false}
            onChange={(enabled) => edit({ click_removal: { ...clicks, enabled } })}
          />
          <SliderField
            label="Click threshold"
            disabled={!(clicks.enabled ?? false)}
            min={AUDIO_LIMITS.clickThreshold.min}
            max={AUDIO_LIMITS.clickThreshold.max}
            step={0.5}
            value={clickSlider.pending ?? clickThreshold}
            onChange={clickSlider.change}
            readout={
              <>
                {(clickSlider.pending ?? clickThreshold).toFixed(1)}
                <span className="text-ink-faint">×</span>
              </>
            }
          />
        </SettingRow>

        <SettingRow label="Denoise">
          <Checkbox
            label="Noise reduction"
            checked={denoise.enabled ?? false}
            onChange={(enabled) => edit({ denoise: { ...denoise, enabled } })}
          />
          <Segmented
            label="Noise reduction mode"
            value={denoise.mode ?? "spectral"}
            options={DENOISE_MODES}
            onChange={(mode) => edit({ denoise: { ...denoise, mode } })}
          />
          <SliderField
            label="Noise reduction strength"
            disabled={!(denoise.enabled ?? false)}
            min={0}
            max={1}
            step={0.05}
            value={denoiseSlider.pending ?? denoiseStrength}
            onChange={denoiseSlider.change}
            readout={
              <>
                {Math.round((denoiseSlider.pending ?? denoiseStrength) * 100)}
                <span className="text-ink-faint">%</span>
              </>
            }
          />
        </SettingRow>

        <SettingRow label="Auto notch" title="Finds and removes steady carriers">
          <Checkbox
            label="Automatic notch"
            checked={audio.auto_notch ?? false}
            onChange={(auto_notch) => edit({ auto_notch })}
          />
        </SettingRow>

        <SettingRow label="Passband">
          <Checkbox
            label="Audio filter"
            checked={filter.enabled ?? false}
            onChange={(enabled) => edit({ filter: { ...filter, enabled } })}
          />
          <NumberField
            label="Audio filter low cut"
            unit="Hz"
            value={lowHz}
            min={AUDIO_LIMITS.toneHz.min}
            max={AUDIO_LIMITS.toneHz.max}
            step={10}
            invalid={lowHz >= highHz}
            className="w-20"
            onCommit={(low_hz) => edit({ filter: { ...filter, low_hz } })}
          />
          <span className="legend">–</span>
          <NumberField
            label="Audio filter high cut"
            unit="Hz"
            value={highHz}
            min={AUDIO_LIMITS.toneHz.min}
            max={AUDIO_LIMITS.toneHz.max}
            step={10}
            invalid={lowHz >= highHz}
            className="w-20"
            onCommit={(high_hz) => edit({ filter: { ...filter, high_hz } })}
          />
        </SettingRow>
      </SettingGroup>

      {notches.map((notch, index) => (
        <NotchRow
          key={`notch-${index}`}
          index={index}
          notch={notch}
          onEdit={(patch) => edit({ notches: withNotchAt(notches, index, patch) })}
          onRemove={() => edit({ notches: withNotchRemoved(notches, index) })}
        />
      ))}
    </>
  );
}

function NotchRow({
  index,
  notch,
  onEdit,
  onRemove,
}: {
  index: number;
  notch: NotchSettings;
  onEdit: (patch: Partial<NotchSettings>) => void;
  onRemove: () => void;
}) {
  return (
    <SettingRow label={`Notch ${index + 1}`}>
      <NumberField
        label={`Notch ${index + 1} frequency`}
        value={notch.freq_hz ?? AUDIO_DEFAULTS.notchFreqHz}
        min={AUDIO_LIMITS.toneHz.min}
        max={AUDIO_LIMITS.toneHz.max}
        step={10}
        className="w-28"
        unit="Hz"
        onCommit={(freq_hz) => onEdit({ freq_hz })}
      />
      <NumberField
        label={`Notch ${index + 1} width`}
        value={notch.width_hz ?? AUDIO_DEFAULTS.notchWidthHz}
        min={AUDIO_LIMITS.notchWidthHz.min}
        max={AUDIO_LIMITS.notchWidthHz.max}
        step={10}
        className="w-28"
        unit="wide"
        onCommit={(width_hz) => onEdit({ width_hz })}
      />
      <Button
        type="button"
        className={`${ICON_BTN_SM} ml-auto hover:text-danger`}
        aria-label={`Remove notch ${index + 1}`}
        onClick={onRemove}
      >
        <Icon glyph={X} size={12} />
      </Button>
    </SettingRow>
  );
}
