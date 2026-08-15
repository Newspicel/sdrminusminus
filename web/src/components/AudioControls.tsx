// The audio chain every voice channel carries. One panel rather than a per-mode control, for the
// same reason the engine hosts it once: a blanker and a notch are the same things on NFM as on
// SSB, and an operator who learns them here knows them everywhere.
import type { AudioAgcMode, AudioProcessing, ChannelSettings, NotchSettings } from "../lib/types";
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
import { NumberField } from "./NumberField";
import { Segmented } from "./Segmented";
import { SettingGroup, SettingRow } from "./Settings";
import { Slider } from "./Slider";
import { useDebouncedCommit } from "./useDebouncedCommit";

const AGC_MODES: Options<AudioAgcMode> = [
  { value: "off", label: "Off" },
  { value: "slow", label: "Slow" },
  { value: "medium", label: "Med" },
  { value: "fast", label: "Fast" },
];

export function AudioControls({
  settings,
  onAudio,
}: {
  settings: ChannelSettings;
  onAudio: (audio: AudioProcessing) => void;
}) {
  const audio = settings.audio ?? {};
  const notches = audio.notches ?? [];
  const edit = (patch: Partial<AudioProcessing>) => onAudio(mergeAudio(settings, patch));

  const blanker = audio.blanker ?? {};
  const blankerThreshold = blanker.threshold ?? AUDIO_DEFAULTS.blankerThreshold;
  const blankerSlider = useDebouncedCommit((threshold: number) =>
    edit({ blanker: { ...blanker, threshold } }),
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
            {/* Several stages are rows further down or collapsed behind a checkbox, so the
                block says whether anything at all is being done to the audio. */}
            {audioChainActive(audio) && <span className="text-accent"> on</span>}
          </>
        }
        // Level with the block's name rather than on a row of its own: adding a notch is
        // something done *to* the chain, and a row would cost every face the height whether or
        // not it ever holds one.
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
            + notch
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

        <SettingRow label="Blanker">
          <Checkbox
            label="Noise blanker"
            checked={blanker.enabled ?? false}
            onChange={(enabled) => edit({ blanker: { ...blanker, enabled } })}
          />
          {/* Drawn whether or not the stage is on, so switching it does not resize the face
              under the pointer — off, this is the threshold it will cut at. */}
          <Slider
            label="Noise blanker threshold"
            className="min-w-0 flex-1"
            disabled={!(blanker.enabled ?? false)}
            min={AUDIO_LIMITS.blankerThreshold.min}
            max={AUDIO_LIMITS.blankerThreshold.max}
            step={0.5}
            value={blankerSlider.pending ?? blankerThreshold}
            onChange={blankerSlider.change}
          />
          <span className="w-10 shrink-0 text-right font-mono text-xs tabular-nums">
            {(blankerSlider.pending ?? blankerThreshold).toFixed(1)}
            <span className="text-ink-faint">×</span>
          </span>
        </SettingRow>

        <SettingRow label="Denoise">
          <Checkbox
            label="Spectral noise reduction"
            checked={denoise.enabled ?? false}
            onChange={(enabled) => edit({ denoise: { ...denoise, enabled } })}
          />
          <Slider
            label="Noise reduction strength"
            className="min-w-0 flex-1"
            disabled={!(denoise.enabled ?? false)}
            min={0}
            max={1}
            step={0.05}
            value={denoiseSlider.pending ?? denoiseStrength}
            onChange={denoiseSlider.change}
          />
          <span className="w-10 shrink-0 text-right font-mono text-xs tabular-nums">
            {Math.round((denoiseSlider.pending ?? denoiseStrength) * 100)}
            <span className="text-ink-faint">%</span>
          </span>
        </SettingRow>

        <SettingRow label="Auto notch">
          <Checkbox
            label="Automatic notch"
            checked={audio.auto_notch ?? false}
            onChange={(auto_notch) => edit({ auto_notch })}
          />
          <span className="text-xs text-ink-dim">finds steady carriers by itself</span>
        </SettingRow>

        <SettingRow label="Passband">
          <Checkbox
            label="Audio filter"
            checked={filter.enabled ?? false}
            onChange={(enabled) => edit({ filter: { ...filter, enabled } })}
          />
          <NumberField
            label="Audio filter low cut (Hz)"
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
            label="Audio filter high cut (Hz)"
            value={highHz}
            min={AUDIO_LIMITS.toneHz.min}
            max={AUDIO_LIMITS.toneHz.max}
            step={10}
            invalid={lowHz >= highHz}
            className="w-20"
            onCommit={(high_hz) => edit({ filter: { ...filter, high_hz } })}
          />
          <span className="legend">Hz</span>
        </SettingRow>
      </SettingGroup>

      {notches.map((notch, index) => (
        <NotchRow
          // A notch has no identity beyond where it sits in the list, which is also what the
          // engine builds its filters from.
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

/** One notch on one row: two numbers and a way to take it off again. A group with a heading per
 * notch would be three rows for two fields, and four of those is most of a face. */
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
        label={`Notch ${index + 1} frequency (Hz)`}
        value={notch.freq_hz ?? AUDIO_DEFAULTS.notchFreqHz}
        min={AUDIO_LIMITS.toneHz.min}
        max={AUDIO_LIMITS.toneHz.max}
        step={10}
        className="w-20"
        onCommit={(freq_hz) => onEdit({ freq_hz })}
      />
      <span className="legend">Hz wide</span>
      <NumberField
        label={`Notch ${index + 1} width (Hz)`}
        value={notch.width_hz ?? AUDIO_DEFAULTS.notchWidthHz}
        min={AUDIO_LIMITS.notchWidthHz.min}
        max={AUDIO_LIMITS.notchWidthHz.max}
        step={10}
        className="w-20"
        onCommit={(width_hz) => onEdit({ width_hz })}
      />
      <Button
        type="button"
        className={`${ICON_BTN_SM} ml-auto hover:text-danger`}
        aria-label={`Remove notch ${index + 1}`}
        onClick={onRemove}
      >
        ✕
      </Button>
    </SettingRow>
  );
}
