import { useState } from "react";
import type { ChannelDescriptor, ChannelInfo, ChannelParams } from "../lib/types";
import { type ChannelEdit, useChannelPatch } from "../lib/useChannelPatch";
import { Checkbox } from "./Checkbox";
import { type ChannelParamsOf, clampOffsetHz, offsetLimitHz } from "./channelSettings";
import { BTN, CHECK_LABEL, LABEL, type Options } from "./controls";
import { formatKhz } from "./format";
import { NumberField, OptionalNumberField } from "./NumberField";
import { Segmented } from "./Segmented";
import { Select, withCurrent } from "./Select";
import { Slider } from "./Slider";
import { useDebouncedCommit } from "./useDebouncedCommit";

const OFFSET_STEPS_HZ = [-25_000, -5_000, 5_000, 25_000];
const DEFAULT_SQUELCH_DB = -60;

// Choice lists for the wire enums, typed off the generated union so a renamed or added variant
// breaks here instead of shipping an option the server rejects.
/** Which DMR timeslot reaches the log and audio output; the receiver always hears both. */
const DMR_SLOTS: Options<NonNullable<ChannelParamsOf<"dmr">["slots"]>> = [
  { value: "both", label: "Both" },
  { value: "one", label: "TS1" },
  { value: "two", label: "TS2" },
];
/** NXDN's two channel widths, which are two different symbol rates to the demodulator. */
const NXDN_WIDTHS: Options<NonNullable<ChannelParamsOf<"nxdn">["bandwidth"]>> = [
  { value: "narrow", label: "6.25" },
  { value: "wide", label: "12.5" },
];
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
const NFM_TONE_MODES: Options<NonNullable<ChannelParamsOf<"nfm">["tone_mode"]>> = [
  { value: "off", label: "Off" },
  { value: "detect", label: "Detect" },
  { value: "ctcss", label: "CTCSS" },
  { value: "dcs", label: "DCS" },
];
// The 50 standard CTCSS tones and the 83 standard DCS codes, as a radio's own code lists. The
// server refuses anything outside them (it is what the detector searches), so an entry that
// drifted from `channels::tone_squelch` fails loudly the first time it is picked.
const CTCSS_TONES_HZ = [
  67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5, 107.2,
  110.9, 114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 159.8, 162.2, 165.5,
  167.9, 171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6, 199.5, 203.5, 206.5, 210.7,
  218.1, 225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];
const DCS_CODES = [
  23, 25, 26, 31, 32, 43, 47, 51, 54, 65, 71, 72, 73, 74, 114, 115, 116, 125, 131, 132, 134, 143,
  152, 155, 156, 162, 165, 172, 174, 205, 223, 226, 243, 244, 245, 251, 261, 263, 265, 271, 306,
  311, 315, 331, 343, 346, 351, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 464, 465, 466,
  503, 506, 516, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712, 723, 731,
  732, 734, 743, 754,
];
const CTCSS_DEFAULT_HZ = 88.5;
const DCS_DEFAULT_CODE = 23;
const CTCSS_OPTIONS: Options<number> = CTCSS_TONES_HZ.map((hz) => ({
  value: hz,
  label: `${hz.toFixed(1)} Hz`,
}));
const DCS_OPTIONS: Options<number> = DCS_CODES.map((code) => ({
  value: code,
  label: String(code).padStart(3, "0"),
}));
const RTTY_STOP_BITS: Options<NonNullable<ChannelParamsOf<"rtty">["stop_bits"]>> = [
  { value: "one", label: "1" },
  { value: "one_and_half", label: "1.5" },
  { value: "two", label: "2" },
];
const ATV_MODULATIONS: Options<NonNullable<ChannelParamsOf<"atv">["modulation"]>> = [
  { value: "am", label: "AM" },
  { value: "fm", label: "FM" },
];
const ATV_STANDARDS: Options<NonNullable<ChannelParamsOf<"atv">["standard"]>> = [
  { value: "ccir625", label: "625 / 25" },
  { value: "eia525", label: "525 / 30" },
  { value: "system_a405", label: "405 / 25" },
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
const SUBGHZ_MODULATIONS: Options<NonNullable<ChannelParamsOf<"subghz">["modulation"]>> = [
  { value: "ook", label: "OOK/ASK" },
  { value: "fsk", label: "FSK" },
];

/**
 * One channel's settings, addressed by the device set it lives on and the engine channel itself.
 * Everything above — which radio, which node, whether it exists at all — is the caller's; this is
 * only the control surface.
 */
export function ChannelControls({
  deviceSet,
  channel,
  descriptor,
  spanHz,
}: {
  deviceSet: number;
  channel: ChannelInfo;
  descriptor: ChannelDescriptor | undefined;
  /** Receiver sample rate, which is the width the offset may move within. */
  spanHz: number | null;
}) {
  const { applyEdit } = useChannelPatch();
  const onEdit = (edit: ChannelEdit): void => applyEdit(deviceSet, channel.id, edit);
  const settings = channel.settings;
  const offsetHz = settings.offset_hz ?? 0;
  const squelchDb = settings.squelch_db ?? null;
  const [offSquelchDb, setOffSquelchDb] = useState(DEFAULT_SQUELCH_DB);
  const squelchSlider = useDebouncedCommit((db) => onEdit({ squelch_db: db }));
  const limitHz = offsetLimitHz(spanHz, descriptor);
  const limitKhz = limitHz === null ? undefined : limitHz / 1000;

  return (
    <div className="flex flex-col gap-2 p-2">
      <div className="flex flex-wrap items-center gap-1">
        {OFFSET_STEPS_HZ.map((step) => (
          <button
            key={step}
            type="button"
            className={`${BTN} font-mono tabular-nums`}
            onClick={() =>
              onEdit((current) => ({
                offset_hz: clampOffsetHz((current.offset_hz ?? 0) + step, limitHz),
              }))
            }
          >
            {step > 0 ? "+" : "−"}
            {Math.abs(step) / 1000}k
          </button>
        ))}
        <NumberField
          label="Offset (kHz)"
          value={offsetHz / 1000}
          min={limitKhz !== undefined ? -limitKhz : undefined}
          max={limitKhz}
          step={0.5}
          onCommit={(khz) => onEdit({ offset_hz: Math.round(khz * 1000) })}
          className="w-24"
        />
        <span className="legend">kHz</span>
      </div>

      {/* Wrapping, not nowrap: a node face is as narrow as the operator drags it, and the
          threshold readout must stay beside its slider rather than be clipped. */}
      <div className={`${LABEL} flex-wrap`}>
        {/* The label is the box and its word, not the row: with the slider inside it too, a
            click anywhere in the row — the threshold readout included — was forwarded to the
            box and turned squelch off. */}
        <label className={CHECK_LABEL}>
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
        </label>
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
      </div>

      <div className="flex flex-wrap items-center gap-x-5 gap-y-2">
        <ModeControls params={settings.params} onParams={(params) => onEdit({ params })} />
      </div>
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
    case "nfm": {
      const mode = params.settings.tone_mode ?? "off";
      const set = (settings: ChannelParamsOf<"nfm">) => onParams({ type: "nfm", settings });
      return (
        <>
          <label className={LABEL}>
            BW
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 12_500}
              optionsHz={[12_500, 25_000]}
              onCommit={(bandwidth_hz) => set({ ...params.settings, bandwidth_hz })}
            />
          </label>
          <label className={LABEL}>
            Tone
            <Select
              label="Tone squelch"
              value={mode}
              options={NFM_TONE_MODES}
              // Switching to a gating mode without a tone chosen would be settings the server
              // refuses, so the first standard tone and code stand in until one is picked.
              onChange={(tone_mode) =>
                set({
                  ...params.settings,
                  tone_mode,
                  ctcss_hz: params.settings.ctcss_hz ?? CTCSS_DEFAULT_HZ,
                  dcs_code: params.settings.dcs_code ?? DCS_DEFAULT_CODE,
                })
              }
            />
          </label>
          {mode === "ctcss" && (
            <label className={LABEL}>
              CTCSS
              <Select
                label="CTCSS tone"
                value={params.settings.ctcss_hz ?? CTCSS_DEFAULT_HZ}
                options={CTCSS_OPTIONS}
                onChange={(ctcss_hz) => set({ ...params.settings, ctcss_hz })}
              />
            </label>
          )}
          {mode === "dcs" && (
            <label className={LABEL}>
              DCS
              <Select
                label="DCS code"
                value={params.settings.dcs_code ?? DCS_DEFAULT_CODE}
                options={DCS_OPTIONS}
                onChange={(dcs_code) => set({ ...params.settings, dcs_code })}
              />
            </label>
          )}
        </>
      );
    }
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
            label="Stereo"
            checked={params.settings.stereo ?? true}
            onChange={(stereo) =>
              onParams({ type: "wfm", settings: { ...params.settings, stereo } })
            }
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
    case "navtex":
      // 100 baud at a 170 Hz shift is the whole standard (ITU-R M.540); the sideband the
      // receiver landed on is the only thing left to choose.
      return (
        <Toggle
          label="Invert"
          checked={params.settings.invert ?? false}
          onChange={(invert) =>
            onParams({ type: "navtex", settings: { ...params.settings, invert } })
          }
        />
      );
    case "acars":
      return (
        <label className={LABEL}>
          BW
          <BandwidthSelect
            valueHz={params.settings.bandwidth_hz ?? 12_500}
            optionsHz={[8_000, 12_500, 25_000]}
            onCommit={(bandwidth_hz) =>
              onParams({ type: "acars", settings: { ...params.settings, bandwidth_hz } })
            }
          />
        </label>
      );
    case "subghz":
      return (
        <>
          <Segmented
            label="Modulation"
            value={params.settings.modulation ?? "ook"}
            options={SUBGHZ_MODULATIONS}
            onChange={(modulation) =>
              onParams({ type: "subghz", settings: { ...params.settings, modulation } })
            }
          />
          <label className={LABEL}>
            BW
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 150_000}
              optionsHz={[50_000, 100_000, 150_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "subghz", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </label>
          <label className={LABEL}>
            Min pulse
            <NumberField
              label="Shortest keying edge accepted (µs)"
              value={params.settings.min_pulse_us ?? 80}
              min={10}
              max={2_000}
              step={10}
              onCommit={(min_pulse_us) =>
                onParams({ type: "subghz", settings: { ...params.settings, min_pulse_us } })
              }
              className="w-20"
            />
            µs
          </label>
          <label className={LABEL}>
            Frame gap
            <NumberField
              label="Silence that ends a frame (µs)"
              value={params.settings.frame_gap_us ?? 5_000}
              min={500}
              max={100_000}
              step={500}
              onCommit={(frame_gap_us) =>
                onParams({ type: "subghz", settings: { ...params.settings, frame_gap_us } })
              }
              className="w-24"
            />
            µs
          </label>
        </>
      );
    case "atv":
      return (
        <>
          <Segmented
            label="Modulation"
            value={params.settings.modulation ?? "am"}
            options={ATV_MODULATIONS}
            onChange={(modulation) =>
              onParams({ type: "atv", settings: { ...params.settings, modulation } })
            }
          />
          <label className={LABEL}>
            Lines
            <Select
              label="Scanning standard"
              value={params.settings.standard ?? "ccir625"}
              options={ATV_STANDARDS}
              onChange={(standard) =>
                onParams({ type: "atv", settings: { ...params.settings, standard } })
              }
            />
          </label>
          <label className={LABEL}>
            BW
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 1_500_000}
              optionsHz={[500_000, 1_000_000, 1_500_000, 1_600_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "atv", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </label>
          <Toggle
            label="Interlace"
            checked={params.settings.interlace ?? true}
            onChange={(interlace) =>
              onParams({ type: "atv", settings: { ...params.settings, interlace } })
            }
          />
          <Toggle
            label="Invert"
            checked={params.settings.invert ?? false}
            onChange={(invert) =>
              onParams({ type: "atv", settings: { ...params.settings, invert } })
            }
          />
        </>
      );
    case "dmr":
      return (
        <Segmented
          label="Slot"
          value={params.settings.slots ?? "both"}
          options={DMR_SLOTS}
          onChange={(slots) => onParams({ type: "dmr", settings: { ...params.settings, slots } })}
        />
      );
    case "nxdn":
      return (
        <Segmented
          label="Width"
          value={params.settings.bandwidth ?? "narrow"}
          options={NXDN_WIDTHS}
          onChange={(bandwidth) =>
            onParams({ type: "nxdn", settings: { ...params.settings, bandwidth } })
          }
        />
      );
    // Everything about these four — symbol rate, deviation, channel width, sync patterns — is
    // fixed by the mode, so there is nothing to offer beyond the frequency the operator tuned.
    case "dstar":
    case "ysf":
    case "p25":
    case "dpmr":
    case "m17":
      return null;
    default:
      return unhandledMode(params);
  }
}

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
    <label className={CHECK_LABEL}>
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
