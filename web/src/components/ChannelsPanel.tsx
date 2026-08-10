// Channel management for the active device set (PLAN §8, §10): add, tune, squelch, and listen
// per channel. Edits go through `useChannelPatch`, the same optimistic pipeline a marker drag
// and the keyboard use, so sliders don't fight WS-driven refetches and no two surfaces disagree.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { channelTypesQuery, createChannel, deleteChannel, STATE_KEY } from "../lib/api";
import { useChannelAudio } from "../lib/audio/useChannelAudio";
import { pushToast } from "../lib/toasts";
import type {
  ChannelDescriptor,
  ChannelInfo,
  ChannelParams,
  ChannelSettings,
  DeviceSet,
} from "../lib/types";
import { type ChannelEdit, useChannelPatch } from "../lib/useChannelPatch";
import type { SdrSocket } from "../lib/ws";
import { Checkbox } from "./Checkbox";
import {
  type ChannelParamsOf,
  channelDecoderKind,
  channelHasAudio,
  defaultChannelSettings,
} from "./channelSettings";
import { BTN, BTN_DANGER, BTN_PRIMARY, BTN_QUIET, LABEL, type Options, segment } from "./controls";
import { formatKhz, formatSignedKhz } from "./format";
import { NumberField, OptionalNumberField } from "./NumberField";
import { Segmented } from "./Segmented";
import { Select, withCurrent } from "./Select";
import { Slider } from "./Slider";
import { TemplatesPanel } from "./TemplatesPanel";
import { useDebouncedCommit } from "./useDebouncedCommit";

const OFFSET_STEPS_HZ = [-25_000, -5_000, 5_000, 25_000];
const DEFAULT_SQUELCH_DB = -60;

// Choice lists for the wire enums, typed off the generated union so a renamed or added variant
// breaks here instead of shipping an option the server rejects.
const SIDEBANDS: Options<NonNullable<ChannelParamsOf<"ssb">["sideband"]>> = [
  { value: "usb", label: "USB" },
  { value: "lsb", label: "LSB" },
];
const POCSAG_BAUDS: Options<NonNullable<ChannelParamsOf<"pocsag">["baud"]>> = [
  { value: "auto", label: "Auto" },
  { value: "b512", label: "512" },
  { value: "b1200", label: "1200" },
  { value: "b2400", label: "2400" },
];
const AIS_CHANNELS: Options<NonNullable<ChannelParamsOf<"ais">["ais_channel"]>> = [
  { value: "a", label: "A" },
  { value: "b", label: "B" },
];
const APRS_MODES: Options<NonNullable<ChannelParamsOf<"aprs">["mode"]>> = [
  { value: "afsk1200", label: "AFSK 1200" },
  { value: "g3ruh9600", label: "G3RUH 9600" },
];
const RTTY_STOP_BITS: Options<NonNullable<ChannelParamsOf<"rtty">["stop_bits"]>> = [
  { value: "one", label: "1" },
  { value: "one_and_half", label: "1.5" },
  { value: "two", label: "2" },
];
const DEEMPHASIS_US: Options<number> = [
  { value: 50, label: "50 µs" },
  { value: 75, label: "75 µs" },
];
const RTTY_BAUDS: Options<number> = [
  { value: 45.45, label: "45.45" },
  { value: 50, label: "50" },
  { value: 75, label: "75" },
];
const RTTY_SHIFTS_HZ: Options<number> = [
  { value: 170, label: "170" },
  { value: 450, label: "450" },
  { value: 850, label: "850" },
];

export function ChannelsPanel({
  socket,
  deviceSet,
  selected,
  onSelect,
}: {
  socket: SdrSocket;
  deviceSet: DeviceSet;
  selected: number | null;
  onSelect: (ch: number | null) => void;
}) {
  const queryClient = useQueryClient();
  const types = useQuery(channelTypesQuery());
  const { applyEdit } = useChannelPatch();
  const [newType, setNewType] = useState("nfm");

  const invalidateState = (): void => {
    void queryClient.invalidateQueries({ queryKey: STATE_KEY });
  };
  const createMut = useMutation({
    mutationFn: (settings: ChannelSettings) => createChannel(deviceSet.id, settings),
    onSuccess: (id) => onSelect(id),
    onError: (e) => pushToast(e.message),
    onSettled: invalidateState,
  });
  const deleteMut = useMutation({
    mutationFn: (ch: number) => deleteChannel(deviceSet.id, ch),
    onError: (e) => pushToast(e.message),
    onSettled: invalidateState,
  });

  const descriptorOf = (typeId: string): ChannelDescriptor | undefined =>
    types.data?.types.find((t) => t.type_id === typeId);

  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="flex flex-wrap items-center gap-2">
        <label className={LABEL}>
          <span className="legend">Add</span>
          <Select
            label="Channel type"
            className="w-40"
            value={newType}
            options={(types.data?.types ?? []).map((t) => ({ value: t.type_id, label: t.name }))}
            onChange={setNewType}
          />
        </label>
        <button
          type="button"
          className={BTN_PRIMARY}
          disabled={createMut.isPending || defaultChannelSettings(newType) === null}
          onClick={() => {
            const settings = defaultChannelSettings(newType);
            if (settings) {
              createMut.mutate(settings);
            }
          }}
        >
          Add channel
        </button>
      </div>

      {deviceSet.channels.length === 0 ? (
        <div className="flex flex-col gap-2 rounded-md border border-line bg-panel p-3">
          <p className="text-sm text-ink-dim">
            Nothing is being demodulated yet. Add a channel above, or start from a template.
          </p>
          <TemplatesPanel active={deviceSet} />
        </div>
      ) : (
        deviceSet.channels.map((c) => (
          <ChannelRow
            key={c.id}
            socket={socket}
            dsId={deviceSet.id}
            channel={c}
            descriptor={descriptorOf(c.settings.params.type)}
            spanHz={deviceSet.settings.sample_rate ?? null}
            selected={selected === c.id}
            onSelect={() => onSelect(c.id)}
            onEdit={(edit) => applyEdit(deviceSet.id, c.id, edit)}
            onRemove={() => deleteMut.mutate(c.id)}
          />
        ))
      )}
    </div>
  );
}

function ChannelRow({
  socket,
  dsId,
  channel,
  descriptor,
  spanHz,
  selected,
  onSelect,
  onEdit,
  onRemove,
}: {
  socket: SdrSocket;
  dsId: number;
  channel: ChannelInfo;
  descriptor: ChannelDescriptor | undefined;
  spanHz: number | null;
  selected: boolean;
  onSelect: () => void;
  onEdit: (edit: ChannelEdit) => void;
  onRemove: () => void;
}) {
  const typeId = channel.settings.params.type;
  const name = descriptor?.name ?? typeId.toUpperCase();
  const hasAudio = channelHasAudio(descriptor);
  const decoderKind = channelDecoderKind(descriptor);
  // Unconditional (rules of hooks); a data channel simply never starts a stream.
  const audio = useChannelAudio(socket, dsId, channel.id);
  // Any live intent — bound, still subscribing, or muted by a suspended output — must offer
  // Stop, or an in-flight/failed subscribe leaves the button inert (no silent failure).
  const engaged = audio.playing || audio.pending || audio.suspended;
  const settings = channel.settings;
  const offsetHz = settings.offset_hz ?? 0;
  const squelchDb = settings.squelch_db ?? null;
  // Remembered across off/on so re-enabling restores the last threshold.
  const [offSquelchDb, setOffSquelchDb] = useState(DEFAULT_SQUELCH_DB);
  const squelchSlider = useDebouncedCommit((db) => onEdit({ squelch_db: db }));
  const halfSpanKhz = spanHz !== null ? spanHz / 2000 : undefined;

  return (
    <article
      className={`overflow-hidden rounded-md border ${
        selected ? "border-accent bg-panel" : "border-line bg-panel"
      }`}
    >
      <div className="flex items-center gap-2 bg-panel-2 pr-1 pl-1">
        <button
          type="button"
          // The whole name-and-badge run selects, not just the word: width along the axis the
          // pointer travels is the cheapest thing to buy.
          className="flex min-h-8 flex-1 items-center gap-2 px-2 text-left"
          onClick={onSelect}
          aria-pressed={selected}
        >
          <span
            className={`font-mono text-xs font-medium ${selected ? "text-accent" : "text-ink"}`}
          >
            {name}
          </span>
          {decoderKind !== null && (
            <span className="legend rounded-[2px] border border-line px-1">{decoderKind}</span>
          )}
          <span className="font-mono text-xs text-ink-dim tabular-nums">
            {formatSignedKhz(offsetHz)}
          </span>
        </button>

        {hasAudio && (
          <>
            <button
              type="button"
              className={engaged ? segment(true) : BTN_QUIET}
              aria-pressed={engaged}
              onClick={() => (engaged ? audio.stop() : audio.start())}
            >
              {engaged ? "Stop" : "Play"}
            </button>
            <Slider
              label={`${name} volume`}
              className="w-20"
              min={0}
              max={1}
              step={0.02}
              value={audio.volume}
              onChange={audio.setVolume}
            />
          </>
        )}

        <button
          type="button"
          className={`${BTN_QUIET} hover:text-danger`}
          onClick={onRemove}
          aria-label={`Remove ${name} channel`}
        >
          ×
        </button>
      </div>

      <div className="flex flex-wrap items-center gap-x-5 gap-y-2 p-2">
        <div className="flex items-center gap-1">
          {OFFSET_STEPS_HZ.map((step) => (
            <button
              key={step}
              type="button"
              className={`${BTN} font-mono tabular-nums`}
              onClick={() => onEdit((current) => ({ offset_hz: (current.offset_hz ?? 0) + step }))}
            >
              {step > 0 ? "+" : "−"}
              {Math.abs(step) / 1000}k
            </button>
          ))}
          <NumberField
            label="Offset (kHz)"
            value={offsetHz / 1000}
            min={halfSpanKhz !== undefined ? -halfSpanKhz : undefined}
            max={halfSpanKhz}
            step={0.5}
            onCommit={(khz) => onEdit({ offset_hz: Math.round(khz * 1000) })}
            className="w-24"
          />
          <span className="legend">kHz</span>
        </div>

        <label className={LABEL}>
          <Checkbox
            checked={squelchDb !== null}
            onChange={(on) => {
              if (on) {
                onEdit({ squelch_db: offSquelchDb });
              } else {
                setOffSquelchDb(squelchSlider.pending ?? squelchDb ?? DEFAULT_SQUELCH_DB);
                squelchSlider.cancel();
                onEdit({ squelch_db: null });
              }
            }}
          />
          <span className="legend">Squelch</span>
          {squelchDb !== null && (
            <>
              <Slider
                label="Squelch threshold (dB)"
                className="w-24"
                min={-120}
                max={0}
                step={1}
                value={squelchSlider.pending ?? squelchDb}
                onChange={squelchSlider.change}
              />
              <span className="w-12 text-right font-mono text-xs text-ink tabular-nums">
                {(squelchSlider.pending ?? squelchDb).toFixed(0)}
              </span>
            </>
          )}
        </label>

        <ModeControls params={settings.params} onParams={(params) => onEdit({ params })} />
      </div>

      {hasAudio && audio.suspended && (
        <InlineFault
          message="Audio output suspended by the browser — no sound."
          action="Resume"
          onAction={audio.resumeOutput}
        />
      )}
      {hasAudio && audio.error !== null && (
        <InlineFault
          message={`Audio failed: ${audio.error}`}
          action="Dismiss"
          onAction={audio.dismissError}
        />
      )}
    </article>
  );
}

/** A fault bound to one channel stays on that channel — it has coordinates the toast stack
 * cannot show, and the fix is the button beside it. */
function InlineFault({
  message,
  action,
  onAction,
}: {
  message: string;
  action: string;
  onAction: () => void;
}) {
  return (
    <div
      role="alert"
      className="flex items-center justify-between gap-3 border-t border-danger/40 bg-danger/10 px-2 py-1.5"
    >
      <span className="font-mono text-xs text-danger">{message}</span>
      <button type="button" className={BTN_DANGER} onClick={onAction}>
        {action}
      </button>
    </div>
  );
}

function ModeControls({
  params,
  onParams,
}: {
  params: ChannelParams;
  onParams: (params: ChannelParams) => void;
}) {
  switch (params.type) {
    case "nfm":
      return (
        <label className={LABEL}>
          BW
          <BandwidthSelect
            valueHz={params.settings.bandwidth_hz ?? 12_500}
            optionsHz={[12_500, 25_000]}
            onCommit={(bandwidth_hz) =>
              onParams({ type: "nfm", settings: { ...params.settings, bandwidth_hz } })
            }
          />
        </label>
      );
    case "am":
      return (
        <>
          <label className={LABEL}>
            BW
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 10_000}
              optionsHz={[5_000, 8_000, 10_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "am", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </label>
          <Toggle
            label="AGC"
            checked={params.settings.agc ?? true}
            onChange={(agc) => onParams({ type: "am", settings: { ...params.settings, agc } })}
          />
        </>
      );
    case "ssb":
      return (
        <>
          <Segmented
            label="Sideband"
            value={params.settings.sideband ?? "usb"}
            options={SIDEBANDS}
            onChange={(sideband) =>
              onParams({ type: "ssb", settings: { ...params.settings, sideband } })
            }
          />
          <label className={LABEL}>
            BW
            <NumberField
              label="SSB bandwidth (Hz)"
              value={params.settings.bandwidth_hz ?? 2_700}
              min={200}
              max={10_000}
              step={100}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "ssb", settings: { ...params.settings, bandwidth_hz } })
              }
              className="w-20"
            />
            Hz
          </label>
          <Toggle
            label="AGC"
            checked={params.settings.agc ?? true}
            onChange={(agc) => onParams({ type: "ssb", settings: { ...params.settings, agc } })}
          />
        </>
      );
    case "wfm":
      return (
        <>
          <label className={LABEL}>
            De-emphasis
            <Select
              label="De-emphasis (µs)"
              value={params.settings.deemphasis_us ?? 50}
              options={DEEMPHASIS_US}
              onChange={(deemphasis_us) =>
                onParams({ type: "wfm", settings: { ...params.settings, deemphasis_us } })
              }
            />
          </label>
          <Toggle
            label="RDS"
            checked={params.settings.rds ?? false}
            onChange={(rds) => onParams({ type: "wfm", settings: { ...params.settings, rds } })}
          />
        </>
      );
    case "pocsag":
      return (
        <>
          <label className={LABEL}>
            Baud
            <Select
              label="POCSAG baud"
              value={params.settings.baud ?? "auto"}
              options={POCSAG_BAUDS}
              onChange={(baud) =>
                onParams({ type: "pocsag", settings: { ...params.settings, baud } })
              }
            />
          </label>
          <label className={LABEL}>
            BW
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 12_500}
              optionsHz={[12_500, 25_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "pocsag", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </label>
          <Toggle
            label="Invert"
            checked={params.settings.invert ?? false}
            onChange={(invert) =>
              onParams({ type: "pocsag", settings: { ...params.settings, invert } })
            }
          />
        </>
      );
    case "adsb":
      return (
        <>
          <Toggle
            label="CRC fix"
            checked={params.settings.crc_fix ?? true}
            onChange={(crc_fix) =>
              onParams({ type: "adsb", settings: { ...params.settings, crc_fix } })
            }
          />
          <AdsbReference
            lat={params.settings.ref_lat ?? null}
            lon={params.settings.ref_lon ?? null}
            onCommit={(ref_lat, ref_lon) =>
              onParams({ type: "adsb", settings: { ...params.settings, ref_lat, ref_lon } })
            }
          />
        </>
      );
    case "ais":
      return (
        <span className={LABEL}>
          Channel
          <Segmented
            label="AIS channel"
            value={params.settings.ais_channel ?? "a"}
            options={AIS_CHANNELS}
            onChange={(ais_channel) =>
              onParams({ type: "ais", settings: { ...params.settings, ais_channel } })
            }
          />
        </span>
      );
    case "aprs":
      return (
        <>
          <label className={LABEL}>
            Mode
            <Select
              label="APRS mode"
              value={params.settings.mode ?? "afsk1200"}
              options={APRS_MODES}
              onChange={(mode) =>
                onParams({ type: "aprs", settings: { ...params.settings, mode } })
              }
            />
          </label>
          <label className={LABEL}>
            BW
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 12_500}
              optionsHz={[12_500, 25_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "aprs", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </label>
        </>
      );
    case "rtty":
      return (
        <>
          <span className={LABEL}>
            Baud
            <PresetNumberField
              label="RTTY baud"
              value={params.settings.baud ?? 45.45}
              presets={RTTY_BAUDS}
              min={10}
              max={1_200}
              step={0.05}
              onCommit={(baud) =>
                onParams({ type: "rtty", settings: { ...params.settings, baud } })
              }
            />
          </span>
          <span className={LABEL}>
            Shift
            <PresetNumberField
              label="RTTY shift (Hz)"
              value={params.settings.shift_hz ?? 170}
              presets={RTTY_SHIFTS_HZ}
              min={20}
              max={2_000}
              step={5}
              onCommit={(shift_hz) =>
                onParams({ type: "rtty", settings: { ...params.settings, shift_hz } })
              }
            />
            Hz
          </span>
          <label className={LABEL}>
            Stop
            <Select
              label="RTTY stop bits"
              value={params.settings.stop_bits ?? "one_and_half"}
              options={RTTY_STOP_BITS}
              onChange={(stop_bits) =>
                onParams({ type: "rtty", settings: { ...params.settings, stop_bits } })
              }
            />
          </label>
          <Toggle
            label="Invert"
            checked={params.settings.invert ?? false}
            onChange={(invert) =>
              onParams({ type: "rtty", settings: { ...params.settings, invert } })
            }
          />
          <Toggle
            label="Unshift on space"
            checked={params.settings.unshift_on_space ?? true}
            onChange={(unshift_on_space) =>
              onParams({ type: "rtty", settings: { ...params.settings, unshift_on_space } })
            }
          />
        </>
      );
    case "morse":
      return (
        <>
          <label className={LABEL}>
            BW
            <NumberField
              label="CW filter bandwidth (Hz)"
              value={params.settings.bandwidth_hz ?? 400}
              min={50}
              max={3_000}
              step={50}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "morse", settings: { ...params.settings, bandwidth_hz } })
              }
            />
            Hz
          </label>
          <label className={LABEL}>
            WPM
            <OptionalNumberField
              label="Morse speed (WPM), empty to auto-track"
              placeholder="auto"
              value={params.settings.wpm ?? null}
              min={5}
              max={60}
              step={1}
              onCommit={(wpm) => onParams({ type: "morse", settings: { ...params.settings, wpm } })}
            />
          </label>
        </>
      );
    default:
      return unhandledMode(params);
  }
}

// A new `ChannelParams` variant fails to compile until `ModeControls` gives it a form.
function unhandledMode(_params: never): null {
  return null;
}

function BandwidthSelect({
  valueHz,
  optionsHz,
  onCommit,
}: {
  valueHz: number;
  optionsHz: readonly number[];
  onCommit: (hz: number) => void;
}) {
  const options = withCurrent(
    valueHz,
    optionsHz.map((hz) => ({ value: hz, label: formatKhz(hz) })),
    formatKhz,
  );
  return <Select label="Channel bandwidth" value={valueHz} options={options} onChange={onCommit} />;
}

function Toggle({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className={LABEL}>
      <Checkbox checked={checked} onChange={onChange} />
      {label}
    </label>
  );
}

// The presets are what operators actually use; the field stays free so an off-list value is
// still reachable (and an incoming one is still shown, with no preset marked).
function PresetNumberField({
  label,
  value,
  presets,
  min,
  max,
  step,
  onCommit,
}: {
  label: string;
  value: number;
  presets: Options<number>;
  min: number;
  max: number;
  step: number;
  onCommit: (value: number) => void;
}) {
  return (
    <span className="flex items-center gap-1">
      <Segmented label={`${label} presets`} value={value} options={presets} onChange={onCommit} />
      <NumberField
        label={label}
        value={value}
        min={min}
        max={max}
        step={step}
        onCommit={onCommit}
      />
    </span>
  );
}

// The pair is meaningless half-set (the decoder needs a full reference position), so both
// fields commit together and a half-filled draft is held back as invalid instead.
function AdsbReference({
  lat,
  lon,
  onCommit,
}: {
  lat: number | null;
  lon: number | null;
  onCommit: (lat: number | null, lon: number | null) => void;
}) {
  const [draft, setDraft] = useState<Reference | null>(null);
  const [geoError, setGeoError] = useState<string | null>(null);
  const shown = draft ?? { lat, lon };
  // The fields clamp to their own range, so the only invalid state left is half-filled.
  const valid = (shown.lat === null) === (shown.lon === null);

  // Called when either field commits: the pair is what is patched, so a half-filled draft is
  // simply kept on screen until the other half arrives.
  const commit = (next: Reference): void => {
    if ((next.lat === null) !== (next.lon === null)) {
      setDraft(next);
      return;
    }
    setDraft(null);
    if (next.lat !== lat || next.lon !== lon) {
      onCommit(next.lat, next.lon);
    }
  };

  // Never geolocate on our own — only this button asks the browser, which is what triggers the
  // permission prompt.
  const locate = (): void => {
    if (!navigator.geolocation) {
      setGeoError("no geolocation in this browser");
      return;
    }
    navigator.geolocation.getCurrentPosition(
      (pos) => {
        setGeoError(null);
        setDraft(null);
        onCommit(Number(pos.coords.latitude.toFixed(5)), Number(pos.coords.longitude.toFixed(5)));
      },
      (err) => setGeoError(err.message),
    );
  };

  return (
    <span className="flex flex-wrap items-center gap-1">
      <span className="text-sm text-ink-dim">Ref</span>
      <OptionalNumberField
        label="Reference latitude"
        placeholder="lat"
        className="w-24"
        value={shown.lat}
        min={-90}
        max={90}
        step={REFERENCE_STEP}
        invalid={!valid}
        onCommit={(next) => commit({ ...shown, lat: next })}
      />
      <OptionalNumberField
        label="Reference longitude"
        placeholder="lon"
        className="w-24"
        value={shown.lon}
        min={-180}
        max={180}
        step={REFERENCE_STEP}
        invalid={!valid}
        onCommit={(next) => commit({ ...shown, lon: next })}
      />
      <button type="button" className={BTN} onClick={locate}>
        Use my location
      </button>
      {!valid && <span className="text-sm text-danger">set both</span>}
      {geoError !== null && <span className="text-sm text-danger">{geoError}</span>}
    </span>
  );
}

interface Reference {
  lat: number | null;
  lon: number | null;
}

/** ~1 m at the equator — finer than the decoder's local-position solution needs, and the
 * precision the field is allowed to display (`fractionDigits`). */
const REFERENCE_STEP = 0.00001;
