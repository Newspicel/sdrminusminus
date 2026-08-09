// Channel management for the active device set (PLAN §8, §10): add, tune, squelch, and listen
// per channel. Edits PATCH the full `ChannelSettings` with the same optimistic-cache contract
// as `useDevicePatch`, so sliders don't fight WS-driven refetches.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  channelTypesQuery,
  createChannel,
  deleteChannel,
  patchChannel,
  STATE_KEY,
} from "../lib/api";
import { useChannelAudio } from "../lib/audio/useChannelAudio";
import type {
  ChannelDescriptor,
  ChannelInfo,
  ChannelParams,
  ChannelSettings,
  DeviceSet,
  StateSnapshot,
} from "../lib/types";
import type { SdrSocket } from "../lib/ws";
import {
  type ChannelParamsOf,
  channelDecoderKind,
  channelHasAudio,
  defaultChannelSettings,
  mergeChannelSettings,
} from "./channelSettings";
import { BTN, FIELD } from "./controls";
import { formatKhz } from "./format";
import { NumberField } from "./NumberField";
import { useDebouncedCommit } from "./useDebouncedCommit";

const LABEL = "flex items-center gap-2 text-sm text-ink-dim";
const OFFSET_STEPS_HZ = [-25_000, -5_000, 5_000, 25_000];
const DEFAULT_SQUELCH_DB = -60;

// Choice lists for the wire enums, typed off the generated union so a renamed or added variant
// breaks here instead of shipping an option the server rejects.
type Options<T extends string> = readonly { value: T; label: string }[];

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
const RTTY_BAUDS = [45.45, 50, 75];
const RTTY_SHIFTS_HZ = [170, 450, 850];

type ChannelEdit =
  | Partial<ChannelSettings>
  | ((current: ChannelSettings) => Partial<ChannelSettings>);

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
  const [newType, setNewType] = useState("nfm");
  const [error, setError] = useState<string | null>(null);

  const invalidateState = (): void => {
    void queryClient.invalidateQueries({ queryKey: STATE_KEY });
  };
  const createMut = useMutation({
    mutationFn: (settings: ChannelSettings) => createChannel(deviceSet.id, settings),
    onSuccess: (id) => {
      setError(null);
      onSelect(id);
    },
    onError: (e) => setError(e.message),
    onSettled: invalidateState,
  });
  const deleteMut = useMutation({
    mutationFn: (ch: number) => deleteChannel(deviceSet.id, ch),
    onError: (e) => setError(e.message),
    onSettled: invalidateState,
  });
  const patchMut = useMutation({
    mutationFn: (v: { ch: number; settings: ChannelSettings }) =>
      patchChannel(deviceSet.id, v.ch, v.settings),
    onSuccess: () => setError(null),
    // A rejected PATCH must be visible, not just snap the control back (CLAUDE.md: no silent
    // failure).
    onError: (e) => setError(e.message),
    onSettled: invalidateState,
  });

  // Same optimistic contract as `useDevicePatch`: cancel racing refetches, write the merged
  // settings synchronously so rapid edits accumulate, then PATCH the full object. The
  // function-edit form reads the optimistic value, so step buttons chain correctly.
  const applyEdit = (ch: number, edit: ChannelEdit): void => {
    void queryClient.cancelQueries({ queryKey: STATE_KEY });
    const prev = queryClient.getQueryData<StateSnapshot>(STATE_KEY);
    const current = prev?.device_sets
      .find((d) => d.id === deviceSet.id)
      ?.channels.find((c) => c.id === ch)?.settings;
    if (!prev || !current) {
      return;
    }
    const settings = mergeChannelSettings(
      current,
      typeof edit === "function" ? edit(current) : edit,
    );
    queryClient.setQueryData<StateSnapshot>(STATE_KEY, {
      ...prev,
      device_sets: prev.device_sets.map((d) =>
        d.id === deviceSet.id
          ? { ...d, channels: d.channels.map((c) => (c.id === ch ? { ...c, settings } : c)) }
          : d,
      ),
    });
    patchMut.mutate({ ch, settings });
  };

  const descriptorOf = (typeId: string): ChannelDescriptor | undefined =>
    types.data?.types.find((t) => t.type_id === typeId);

  return (
    <div className="flex flex-col gap-2 px-4 py-3">
      <div className="flex flex-wrap items-center gap-2">
        <label className={LABEL}>
          Type
          <select
            className={FIELD}
            value={newType}
            onChange={(e) => setNewType(e.target.value)}
            aria-label="Channel type"
          >
            {(types.data?.types ?? []).map((t) => (
              <option key={t.type_id} value={t.type_id}>
                {t.name}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          className={BTN}
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

      {error !== null && (
        <div
          role="alert"
          className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          <span>Rejected: {error}</span>
          <button type="button" className="shrink-0 underline" onClick={() => setError(null)}>
            dismiss
          </button>
        </div>
      )}

      {deviceSet.channels.length === 0 ? (
        <span className="text-sm text-ink-dim">No channels — add one to listen.</span>
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
            onEdit={(edit) => applyEdit(c.id, edit)}
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
    <div
      className={`flex flex-col gap-2 rounded border bg-panel px-3 py-2 ${
        selected ? "border-accent" : "border-line"
      }`}
    >
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <button
          type="button"
          className={`font-mono text-sm font-semibold max-md:min-h-10 ${
            selected ? "text-accent" : "text-ink"
          }`}
          onClick={onSelect}
          aria-pressed={selected}
        >
          {name}
        </button>
        {decoderKind !== null && (
          <span className="rounded border border-line px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide text-ink-dim">
            {decoderKind}
          </span>
        )}

        <div className="flex flex-wrap items-center gap-1">
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
          <span className="text-sm text-ink-dim">kHz</span>
        </div>

        {hasAudio && (
          <>
            <button
              type="button"
              className={`${BTN} ${audio.playing ? "border-accent text-accent" : ""}`}
              onClick={() => (engaged ? audio.stop() : audio.start())}
            >
              {engaged ? "Stop" : "Play"}
            </button>
            <label className={LABEL}>
              Vol
              <input
                type="range"
                className="w-20 accent-accent"
                min={0}
                max={1}
                step={0.02}
                value={audio.volume}
                onChange={(e) => audio.setVolume(Number(e.target.value))}
                aria-label="Volume"
              />
            </label>
          </>
        )}

        <button
          type="button"
          className={`${BTN} ml-auto hover:border-danger hover:text-danger`}
          onClick={onRemove}
        >
          Remove
        </button>
      </div>

      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <label className={LABEL}>
          <input
            type="checkbox"
            className="accent-accent"
            checked={squelchDb !== null}
            onChange={(e) => {
              if (e.target.checked) {
                onEdit({ squelch_db: offSquelchDb });
              } else {
                setOffSquelchDb(squelchSlider.pending ?? squelchDb ?? DEFAULT_SQUELCH_DB);
                squelchSlider.cancel();
                onEdit({ squelch_db: null });
              }
            }}
          />
          Squelch
        </label>
        {squelchDb !== null && (
          <label className={LABEL}>
            <input
              type="range"
              className="w-28 accent-accent"
              min={-120}
              max={0}
              step={1}
              value={squelchSlider.pending ?? squelchDb}
              onChange={(e) => squelchSlider.change(Number(e.target.value))}
              aria-label="Squelch threshold (dB)"
            />
            <span className="w-14 text-right font-mono tabular-nums text-ink">
              {(squelchSlider.pending ?? squelchDb).toFixed(0)}{" "}
              <span className="text-ink-dim">dB</span>
            </span>
          </label>
        )}

        <ModeControls params={settings.params} onParams={(params) => onEdit({ params })} />
      </div>

      {hasAudio && audio.suspended && (
        <div
          role="alert"
          className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          <span>Audio output suspended by the browser — no sound.</span>
          <button type="button" className="shrink-0 underline" onClick={audio.resumeOutput}>
            resume
          </button>
        </div>
      )}
      {hasAudio && audio.error !== null && (
        <div
          role="alert"
          className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          <span>Audio failed: {audio.error}</span>
          <button type="button" className="shrink-0 underline" onClick={audio.dismissError}>
            dismiss
          </button>
        </div>
      )}
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
            <select
              className={FIELD}
              value={params.settings.deemphasis_us ?? 50}
              onChange={(e) =>
                onParams({
                  type: "wfm",
                  settings: { ...params.settings, deemphasis_us: Number(e.target.value) },
                })
              }
              aria-label="De-emphasis (µs)"
            >
              <option value={50}>50 µs</option>
              <option value={75}>75 µs</option>
            </select>
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
            <OptionSelect
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
            <OptionSelect
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
            <OptionSelect
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
  optionsHz: number[];
  onCommit: (hz: number) => void;
}) {
  return (
    <select
      className={FIELD}
      value={valueHz}
      onChange={(e) => onCommit(Number(e.target.value))}
      aria-label="Channel bandwidth"
    >
      {/* A preset can carry an off-list bandwidth; render it as selectable so the select
          doesn't lie (same rule as the device BW select). */}
      {!optionsHz.includes(valueHz) && (
        <option value={valueHz}>{formatKhz(valueHz)} (current)</option>
      )}
      {optionsHz.map((hz) => (
        <option key={hz} value={hz}>
          {formatKhz(hz)}
        </option>
      ))}
    </select>
  );
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
      <input
        type="checkbox"
        className="accent-accent"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      {label}
    </label>
  );
}

// Matching the option back by value keeps the enum's generated string literal type — the DOM
// only ever hands back `string`.
function OptionSelect<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: Options<T>;
  onChange: (value: T) => void;
}) {
  return (
    <select
      className={FIELD}
      value={value}
      aria-label={label}
      onChange={(e) => {
        const picked = options.find((o) => o.value === e.target.value);
        if (picked) {
          onChange(picked.value);
        }
      }}
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: Options<T>;
  onChange: (value: T) => void;
}) {
  return (
    <div
      className="flex overflow-hidden rounded border border-line"
      role="group"
      aria-label={label}
    >
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          className={`px-2.5 py-1 font-mono text-sm transition-colors max-md:min-h-10 ${
            value === o.value ? "bg-panel-2 text-accent" : "text-ink-dim hover:text-ink"
          }`}
          aria-pressed={value === o.value}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

// The presets are what operators actually use; the field stays free so an off-list value is
// still reachable (and an incoming one is still shown).
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
  presets: readonly number[];
  min: number;
  max: number;
  step: number;
  onCommit: (value: number) => void;
}) {
  return (
    <span className="flex items-center gap-1">
      <div
        className="flex overflow-hidden rounded border border-line"
        role="group"
        aria-label={`${label} presets`}
      >
        {presets.map((preset) => (
          <button
            key={preset}
            type="button"
            className={`px-2 py-1 font-mono text-sm tabular-nums transition-colors max-md:min-h-10 ${
              value === preset ? "bg-panel-2 text-accent" : "text-ink-dim hover:text-ink"
            }`}
            aria-pressed={value === preset}
            onClick={() => onCommit(preset)}
          >
            {preset}
          </button>
        ))}
      </div>
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

// `NumberField` cannot express "cleared", and for these settings an empty field is a real value
// (auto) rather than a rejected edit — otherwise there is no way back to auto once a speed is set.
function OptionalNumberField({
  label,
  placeholder,
  value,
  min,
  max,
  step,
  onCommit,
}: {
  label: string;
  placeholder: string;
  value: number | null;
  min: number;
  max: number;
  step: number;
  onCommit: (value: number | null) => void;
}) {
  const [text, setText] = useState<string | null>(null);

  const commit = (): void => {
    if (text === null) {
      return;
    }
    setText(null);
    if (text.trim() === "") {
      if (value !== null) {
        onCommit(null);
      }
      return;
    }
    const entered = Number(text);
    if (!Number.isFinite(entered)) {
      return;
    }
    const clamped = Math.min(max, Math.max(min, entered));
    if (clamped !== value) {
      onCommit(clamped);
    }
  };

  return (
    <input
      type="number"
      inputMode="decimal"
      className={`${FIELD} w-20 tabular-nums`}
      aria-label={label}
      placeholder={placeholder}
      value={text ?? (value === null ? "" : String(value))}
      min={min}
      max={max}
      step={step}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          commit();
        } else if (e.key === "Escape") {
          setText(null);
        }
      }}
    />
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
  const [draft, setDraft] = useState<{ lat: string; lon: string } | null>(null);
  const [geoError, setGeoError] = useState<string | null>(null);
  const shown = draft ?? {
    lat: lat === null ? "" : String(lat),
    lon: lon === null ? "" : String(lon),
  };
  const cleared = shown.lat.trim() === "" && shown.lon.trim() === "";
  const parsed = {
    lat: Number(shown.lat),
    lon: Number(shown.lon),
  };
  const valid =
    cleared ||
    (Number.isFinite(parsed.lat) &&
      Math.abs(parsed.lat) <= 90 &&
      shown.lat.trim() !== "" &&
      Number.isFinite(parsed.lon) &&
      Math.abs(parsed.lon) <= 180 &&
      shown.lon.trim() !== "");

  const edit = (next: { lat: string; lon: string }): void => {
    setDraft(next);
  };
  const commit = (): void => {
    if (draft === null || !valid) {
      return;
    }
    setDraft(null);
    if (cleared) {
      if (lat !== null || lon !== null) {
        onCommit(null, null);
      }
      return;
    }
    if (parsed.lat !== lat || parsed.lon !== lon) {
      onCommit(parsed.lat, parsed.lon);
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
      {(["lat", "lon"] as const).map((axis) => (
        <input
          key={axis}
          type="number"
          inputMode="decimal"
          className={`${FIELD} w-24 tabular-nums ${valid ? "" : "border-danger"}`}
          aria-label={axis === "lat" ? "Reference latitude" : "Reference longitude"}
          aria-invalid={!valid}
          placeholder={axis}
          value={shown[axis]}
          step={0.00001}
          onChange={(e) => edit({ ...shown, [axis]: e.target.value })}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              commit();
            } else if (e.key === "Escape") {
              setDraft(null);
            }
          }}
        />
      ))}
      <button type="button" className={BTN} onClick={locate}>
        Use my location
      </button>
      {!valid && <span className="text-sm text-danger">set both</span>}
      {geoError !== null && <span className="text-sm text-danger">{geoError}</span>}
    </span>
  );
}
