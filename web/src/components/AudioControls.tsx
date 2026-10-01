import { Plus, X } from "lucide-react";
import type { AudioAgcMode, AudioProcessing, DenoiseMode, NotchSettings } from "../lib/types";
import { Button } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import {
  AUDIO_DEFAULTS,
  AUDIO_LIMITS,
  mergeAudio,
  withNotchAdded,
  withNotchAt,
  withNotchRemoved,
} from "./channelSettings";
import { CHIP_SETTING, ICON_BTN_SM, type Options } from "./controls";
import { ChipField, ChoiceChip, SettingChip, ToggleChip } from "./face/Chips";
import { formatHz } from "./format";
import { Icon } from "./Icon";
import { NumberField } from "./NumberField";
import { Segmented } from "./Segmented";
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

type Edit = (patch: Partial<AudioProcessing>) => void;

export function AudioControls({
  audio,
  onAudio,
}: {
  audio: AudioProcessing;
  onAudio: (audio: AudioProcessing) => void;
}) {
  const notches = audio.notches ?? [];
  const edit: Edit = (patch) => onAudio(mergeAudio(audio, patch));
  const agc = audio.agc ?? "off";
  return (
    <>
      <ChoiceChip
        label="AGC"
        title="Audio AGC speed"
        value={agc}
        options={AGC_MODES}
        quiet={agc === "off"}
        onChange={(next) => edit({ agc: next })}
      />
      <DeclickChip audio={audio} edit={edit} />
      <DenoiseChip audio={audio} edit={edit} />
      <ToggleChip
        label="Auto notch"
        title="Finds and removes steady carriers"
        on={audio.auto_notch ?? false}
        onChange={(auto_notch) => edit({ auto_notch })}
      />
      <PassbandChip audio={audio} edit={edit} />
      {notches.map((notch, index) => (
        <NotchChip
          key={`notch-${index}`}
          index={index}
          notch={notch}
          onEdit={(patch) => edit({ notches: withNotchAt(notches, index, patch) })}
          onRemove={() => edit({ notches: withNotchRemoved(notches, index) })}
        />
      ))}
      <Button
        type="button"
        className={CHIP_SETTING}
        disabled={notches.length >= AUDIO_LIMITS.maxNotches}
        title={`Add a notch, up to ${AUDIO_LIMITS.maxNotches}`}
        onClick={() => {
          const next = withNotchAdded(notches);
          if (next !== null) {
            edit({ notches: next });
          }
        }}
      >
        <Icon glyph={Plus} size={12} />
        <span className="font-sans">Notch</span>
      </Button>
    </>
  );
}

function DeclickChip({ audio, edit }: { audio: AudioProcessing; edit: Edit }) {
  const clicks = audio.click_removal ?? {};
  const enabled = clicks.enabled ?? false;
  const threshold = clicks.threshold ?? AUDIO_DEFAULTS.clickThreshold;
  const slider = useDebouncedCommit((next: number) =>
    edit({ click_removal: { ...clicks, threshold: next } }),
  );
  const shown = slider.pending ?? threshold;
  return (
    <SettingChip
      label="De-click"
      value={enabled ? `${threshold.toFixed(1)}×` : "off"}
      quiet={!enabled}
      title="Click removal"
    >
      {() => (
        <ChipField label="Click removal">
          <Checkbox
            label="Click removal"
            checked={enabled}
            onChange={(next) => edit({ click_removal: { ...clicks, enabled: next } })}
          />
          <SliderField
            label="Click threshold"
            disabled={!enabled}
            min={AUDIO_LIMITS.clickThreshold.min}
            max={AUDIO_LIMITS.clickThreshold.max}
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

function DenoiseChip({ audio, edit }: { audio: AudioProcessing; edit: Edit }) {
  const denoise = audio.denoise ?? {};
  const enabled = denoise.enabled ?? false;
  const strength = denoise.strength ?? AUDIO_DEFAULTS.denoiseStrength;
  const slider = useDebouncedCommit((next: number) =>
    edit({ denoise: { ...denoise, strength: next } }),
  );
  const shown = slider.pending ?? strength;
  return (
    <SettingChip
      label="Denoise"
      value={enabled ? `${Math.round(strength * 100)}%` : "off"}
      quiet={!enabled}
      title="Noise reduction"
    >
      {() => (
        <ChipField label="Noise reduction">
          <Checkbox
            label="Noise reduction"
            checked={enabled}
            onChange={(next) => edit({ denoise: { ...denoise, enabled: next } })}
          />
          <Segmented
            label="Noise reduction mode"
            value={denoise.mode ?? "spectral"}
            options={DENOISE_MODES}
            onChange={(mode) => edit({ denoise: { ...denoise, mode } })}
          />
          <SliderField
            label="Noise reduction strength"
            disabled={!enabled}
            min={0}
            max={1}
            step={0.05}
            value={shown}
            onChange={slider.change}
            readout={
              <>
                {Math.round(shown * 100)}
                <span className="text-ink-faint">%</span>
              </>
            }
          />
        </ChipField>
      )}
    </SettingChip>
  );
}

function PassbandChip({ audio, edit }: { audio: AudioProcessing; edit: Edit }) {
  const filter = audio.filter ?? {};
  const enabled = filter.enabled ?? false;
  const lowHz = filter.low_hz ?? AUDIO_DEFAULTS.filterLowHz;
  const highHz = filter.high_hz ?? AUDIO_DEFAULTS.filterHighHz;
  const invalid = lowHz >= highHz;
  return (
    <SettingChip
      label="Passband"
      value={enabled ? `${formatHz(lowHz)} – ${formatHz(highHz)}` : "off"}
      quiet={!enabled}
      tone={enabled && invalid ? "danger" : undefined}
      title="Audio filter"
    >
      {() => (
        <ChipField label="Audio filter">
          <Checkbox
            label="Audio filter"
            checked={enabled}
            onChange={(next) => edit({ filter: { ...filter, enabled: next } })}
          />
          <NumberField
            label="Audio filter low cut"
            unit="Hz"
            value={lowHz}
            min={AUDIO_LIMITS.toneHz.min}
            max={AUDIO_LIMITS.toneHz.max}
            step={10}
            invalid={invalid}
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
            invalid={invalid}
            className="w-20"
            onCommit={(high_hz) => edit({ filter: { ...filter, high_hz } })}
          />
        </ChipField>
      )}
    </SettingChip>
  );
}

function NotchChip({
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
  const name = `Notch ${index + 1}`;
  const freqHz = notch.freq_hz ?? AUDIO_DEFAULTS.notchFreqHz;
  return (
    <SettingChip label={name} value={formatHz(freqHz)} title={name}>
      {(close) => (
        <ChipField label={name}>
          <NumberField
            label={`${name} frequency`}
            value={freqHz}
            min={AUDIO_LIMITS.toneHz.min}
            max={AUDIO_LIMITS.toneHz.max}
            step={10}
            className="w-24"
            unit="Hz"
            onCommit={(freq_hz) => onEdit({ freq_hz })}
          />
          <NumberField
            label={`${name} width`}
            value={notch.width_hz ?? AUDIO_DEFAULTS.notchWidthHz}
            min={AUDIO_LIMITS.notchWidthHz.min}
            max={AUDIO_LIMITS.notchWidthHz.max}
            step={10}
            className="w-24"
            unit="wide"
            onCommit={(width_hz) => onEdit({ width_hz })}
          />
          <Button
            type="button"
            className={`${ICON_BTN_SM} ml-auto hover:text-danger`}
            aria-label={`Remove ${name.toLowerCase()}`}
            onClick={() => {
              onRemove();
              close();
            }}
          >
            <Icon glyph={X} size={12} />
          </Button>
        </ChipField>
      )}
    </SettingChip>
  );
}
