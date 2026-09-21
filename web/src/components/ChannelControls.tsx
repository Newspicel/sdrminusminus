import { type ReactNode, useState } from "react";
import type {
  ChannelDescriptor,
  ChannelParams,
  ChannelSettings,
  DecoderEvent,
  ParamLimit,
} from "../lib/types";
import type { ChannelEdit } from "../lib/useChannelPatch";
import { AudioControls } from "./AudioControls";
import { Checkbox } from "./Checkbox";
import {
  AUDIO_DEFAULTS,
  AUDIO_LIMITS,
  type ChannelParamsOf,
  channelHasAudio,
  DEFAULT_SQUELCH_DB,
  limitOf,
  type NumberLimit,
  radioWindowHz,
  SQUELCH_RANGE_DB,
  type SquelchMode,
  scaledLimit,
  squelchAt,
  squelchLevelDb,
  squelchMarginDb,
  squelchMode,
} from "./channelSettings";
import type { Options } from "./controls";
import { inTuningRange, type Range } from "./dial";
import { FrequencyDial } from "./FrequencyDial";
import { formatHz, formatMhz } from "./format";
import { NumberField, OptionalNumberField } from "./NumberField";
import { Segmented } from "./Segmented";
import { Select } from "./Select";
import { SettingRow, Settings } from "./Settings";
import { Slider } from "./Slider";
import { withCurrent } from "./selectOptions";
import { TextAutocomplete } from "./TextAutocomplete";
import { TuneTo } from "./TuneTo";
import { TuningLock } from "./TuningLock";
import { useDebouncedCommit } from "./useDebouncedCommit";

type BroadcastStatus = Extract<DecoderEvent, { kind: "broadcast" }>["data"];

const SQUELCH_MODES: Options<SquelchMode> = [
  { value: "off", label: "Off", title: "Pass everything through" },
  { value: "manual", label: "Manual", title: "Open above a level you set" },
  { value: "auto", label: "Auto", title: "Open a margin above the noise floor it measures" },
];

const DMR_SLOTS: Options<NonNullable<ChannelParamsOf<"dmr">["slots"]>> = [
  { value: "both", label: "Both" },
  { value: "one", label: "TS1" },
  { value: "two", label: "TS2" },
];
const NXDN_WIDTHS: Options<NonNullable<ChannelParamsOf<"nxdn">["bandwidth"]>> = [
  { value: "narrow", label: "6.25" },
  { value: "wide", label: "12.5" },
];
const DECT_BANDS: Options<NonNullable<ChannelParamsOf<"dect">["band"]>> = [
  { value: "eu", label: "EU" },
  { value: "us", label: "US" },
];
const DECT_SIDES: Options<NonNullable<ChannelParamsOf<"dect">["sides"]>> = [
  { value: "both", label: "Both" },
  { value: "rfp", label: "Base" },
  { value: "pp", label: "Handset" },
];
const SIDEBANDS: Options<NonNullable<ChannelParamsOf<"ssb">["sideband"]>> = [
  { value: "usb", label: "USB" },
  { value: "lsb", label: "LSB" },
];
const SELCALL_SYSTEMS: Options<NonNullable<ChannelParamsOf<"selcall">["system"]>> = [
  { value: "ccir1", label: "CCIR-1" },
  { value: "zvei1", label: "ZVEI-1" },
];
const ILS_COMPONENTS: Options<NonNullable<ChannelParamsOf<"ils">["component"]>> = [
  { value: "localizer", label: "Localizer" },
  { value: "glideslope", label: "Glideslope" },
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
const NFM_SCRAMBLER_MODES: Options<NonNullable<ChannelParamsOf<"nfm">["scrambler_mode"]>> = [
  { value: "off", label: "Off" },
  { value: "inversion", label: "Inversion" },
  { value: "auto", label: "Auto" },
];
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
const INVERSION_DEFAULT_HZ = 3_300;
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
const DAB_MODES: Options<NonNullable<ChannelParamsOf<"dab">["mode"]>> = [
  { value: "auto", label: "Auto" },
  { value: "dab", label: "DAB" },
  { value: "dab_plus", label: "DAB+" },
];
const DAB_TRANSMISSION_MODES: Options<NonNullable<ChannelParamsOf<"dab">["transmission_mode"]>> = [
  { value: "i", label: "I" },
  { value: "ii", label: "II" },
  { value: "iii", label: "III" },
  { value: "iv", label: "IV" },
];
const DATV_STANDARDS: Options<NonNullable<ChannelParamsOf<"datv">["standard"]>> = [
  { value: "dvb_s", label: "DVB-S" },
  { value: "dvb_s2", label: "DVB-S2" },
];
const DRM_MODES: Options<NonNullable<ChannelParamsOf<"drm">["mode"]>> = [
  { value: "auto", label: "Auto" },
  { value: "drm30", label: "DRM30" },
  { value: "drm_plus", label: "DRM+" },
];
const SSTV_AUTO = "auto";
const SSTV_MODES: Options<NonNullable<ChannelParamsOf<"sstv">["mode"]> | typeof SSTV_AUTO> = [
  { value: SSTV_AUTO, label: "Follow VIS" },
  { value: "robot36", label: "Robot 36" },
  { value: "robot72", label: "Robot 72" },
  { value: "martin_m1", label: "Martin M1" },
  { value: "martin_m2", label: "Martin M2" },
  { value: "scottie_s1", label: "Scottie S1" },
  { value: "scottie_s2", label: "Scottie S2" },
  { value: "scottie_dx", label: "Scottie DX" },
  { value: "pd50", label: "PD50" },
  { value: "pd90", label: "PD90" },
  { value: "pd120", label: "PD120" },
  { value: "pd180", label: "PD180" },
  { value: "sc2180", label: "Wraase SC2-180" },
];
const ATV_COLORS: Options<NonNullable<ChannelParamsOf<"atv">["color"]>> = [
  { value: "monochrome", label: "Mono" },
  { value: "pal", label: "PAL" },
  { value: "ntsc", label: "NTSC" },
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
const PSK_BAUDS: Options<NonNullable<ChannelParamsOf<"psk">["baud"]>> = [
  { value: "psk31", label: "PSK31" },
  { value: "psk63", label: "PSK63" },
  { value: "psk125", label: "PSK125" },
  { value: "psk250", label: "PSK250" },
];

const RADIO_CLOCK_STANDARDS: Options<NonNullable<ChannelParamsOf<"radio_clock">["standard"]>> = [
  { value: "dcf77", label: "DCF77" },
  { value: "wwvb", label: "WWVB" },
  { value: "msf", label: "MSF" },
  { value: "jjy", label: "JJY" },
];

export function ChannelDial({
  hz,
  descriptor,
  spanHz,
  centerHz,
  range,
  dialId,
  wheelTunes,
  locked,
  onTune,
  onLock,
}: {
  hz: number;
  descriptor: ChannelDescriptor | undefined;
  spanHz: number | null;
  centerHz: number | null;
  range: Range;
  dialId: string;
  wheelTunes: boolean;
  locked: boolean;
  onTune: (hz: number) => void;
  onLock: (locked: boolean) => void;
}) {
  const heard = radioWindowHz(centerHz, spanHz, descriptor);
  return (
    <div className="flex min-w-0 items-center gap-1">
      <FrequencyDial
        id={dialId}
        hz={hz}
        range={range}
        disabled={locked}
        wheelTunes={wheelTunes}
        onTune={onTune}
      />
      <span className="ml-auto flex shrink-0 items-center gap-1">
        <TuneTo
          title="Type a frequency to listen on"
          hz={hz}
          hint={
            heard === null
              ? `Reaches ${formatMhz(range.min)} – ${formatMhz(range.max)}`
              : `The radio hears ${formatMhz(heard.lowHz)} – ${formatMhz(heard.highHz)}`
          }
          resolve={(entered) => inTuningRange(entered, range)}
          disabled={locked}
          onTune={onTune}
        />
        <TuningLock locked={locked} held="Frequency locked" free="Lock frequency" onLock={onLock} />
      </span>
    </div>
  );
}

export function ChannelControls({
  settings,
  descriptor,
  onEdit,
  extra,
  broadcast,
}: {
  settings: ChannelSettings;
  descriptor: ChannelDescriptor | undefined;
  onEdit: (edit: ChannelEdit) => void;
  extra?: ReactNode;
  broadcast?: BroadcastStatus;
}) {
  return (
    <Settings className="p-2">
      {channelHasAudio(descriptor) && <SquelchRow settings={settings} onEdit={onEdit} />}
      <ModeControls
        params={settings.params}
        broadcast={broadcast}
        limits={descriptor?.limits ?? []}
        onParams={(params) => onEdit({ params })}
      />
      {extra}
      {channelHasAudio(descriptor) && (
        <AudioControls settings={settings} onAudio={(audio) => onEdit({ audio })} />
      )}
    </Settings>
  );
}

function SquelchRow({
  settings,
  onEdit,
}: {
  settings: ChannelSettings;
  onEdit: (edit: ChannelEdit) => void;
}) {
  const mode = squelchMode(settings.squelch);
  const [heldDb, setHeldDb] = useState(DEFAULT_SQUELCH_DB);
  const levelSlider = useDebouncedCommit((level_db) =>
    onEdit({ squelch: { mode: "manual", level_db } }),
  );
  const marginSlider = useDebouncedCommit((margin_db) =>
    onEdit({ squelch: { mode: "auto", margin_db } }),
  );
  const levelDb = levelSlider.pending ?? squelchLevelDb(settings.squelch) ?? heldDb;
  const marginDb =
    marginSlider.pending ?? squelchMarginDb(settings.squelch) ?? AUDIO_DEFAULTS.squelchAutoMarginDb;
  const auto = mode === "auto";
  const pick = (next: SquelchMode): void => {
    if (next === mode) {
      return;
    }
    if (mode === "manual") {
      setHeldDb(levelDb);
    }
    levelSlider.cancel();
    marginSlider.cancel();
    onEdit({ squelch: squelchAt(next, { levelDb, marginDb }) });
  };
  return (
    <SettingRow label="Squelch" title="Mute the channel until a signal is strong enough">
      <Segmented label="Squelch mode" value={mode} options={SQUELCH_MODES} onChange={pick} />
      <Slider
        label={auto ? "Squelch margin above the noise floor (dB)" : "Squelch threshold (dB)"}
        className="min-w-0 flex-1"
        disabled={mode === "off"}
        min={auto ? AUDIO_LIMITS.squelchAutoMarginDb.min : SQUELCH_RANGE_DB.min}
        max={auto ? AUDIO_LIMITS.squelchAutoMarginDb.max : SQUELCH_RANGE_DB.max}
        step={1}
        value={auto ? marginDb : levelDb}
        onChange={auto ? marginSlider.change : levelSlider.change}
      />
      <span
        className={`w-14 shrink-0 text-right font-mono text-xs tabular-nums ${
          mode === "off" ? "text-ink-faint opacity-45" : "text-ink"
        }`}
        title={auto ? "Above the noise floor the channel measures" : undefined}
      >
        {auto ? `+${marginDb.toFixed(0)}` : levelDb.toFixed(0)}{" "}
        <span className="text-ink-faint">dB</span>
      </span>
    </SettingRow>
  );
}

function ModeControls({
  broadcast,
  params,
  limits,
  onParams,
}: {
  params: ChannelParams;
  broadcast?: BroadcastStatus;
  limits: readonly ParamLimit[];
  onParams: (params: ChannelParams) => void;
}) {
  switch (params.type) {
    case "nfm": {
      const mode = params.settings.tone_mode ?? "off";
      const scrambler = params.settings.scrambler_mode ?? "off";
      const set = (settings: ChannelParamsOf<"nfm">) => onParams({ type: "nfm", settings });
      return (
        <>
          <SettingRow label="Bandwidth">
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 12_500}
              optionsHz={[12_500, 25_000]}
              onCommit={(bandwidth_hz) => set({ ...params.settings, bandwidth_hz })}
            />
          </SettingRow>
          <SettingRow label="Tone">
            <Select
              label="Tone squelch"
              value={mode}
              options={NFM_TONE_MODES}
              onChange={(tone_mode) =>
                set({
                  ...params.settings,
                  tone_mode,
                  ctcss_hz: params.settings.ctcss_hz ?? CTCSS_DEFAULT_HZ,
                  dcs_code: params.settings.dcs_code ?? DCS_DEFAULT_CODE,
                })
              }
            />
          </SettingRow>
          {mode === "ctcss" && (
            <SettingRow label="CTCSS">
              <Select
                label="CTCSS tone"
                value={params.settings.ctcss_hz ?? CTCSS_DEFAULT_HZ}
                options={CTCSS_OPTIONS}
                onChange={(ctcss_hz) => set({ ...params.settings, ctcss_hz })}
              />
            </SettingRow>
          )}
          {mode === "dcs" && (
            <SettingRow label="DCS">
              <Select
                label="DCS code"
                value={params.settings.dcs_code ?? DCS_DEFAULT_CODE}
                options={DCS_OPTIONS}
                onChange={(dcs_code) => set({ ...params.settings, dcs_code })}
              />
            </SettingRow>
          )}
          <SettingRow label="Scrambler">
            <Select
              label="Voice scrambler"
              value={scrambler}
              options={NFM_SCRAMBLER_MODES}
              onChange={(scrambler_mode) =>
                set({
                  ...params.settings,
                  scrambler_mode,
                  inversion_hz: params.settings.inversion_hz ?? INVERSION_DEFAULT_HZ,
                })
              }
            />
          </SettingRow>
          {scrambler === "inversion" && (
            <SettingRow label="Carrier">
              <NumberField
                label="Inversion carrier (Hz)"
                value={params.settings.inversion_hz ?? INVERSION_DEFAULT_HZ}
                {...limitOf(limits, "inversion_hz")}
                onCommit={(inversion_hz) => set({ ...params.settings, inversion_hz })}
                className="w-20"
              />
              <span className="legend">Hz</span>
            </SettingRow>
          )}
          <Toggle
            label="Compander"
            title="Expand audio that was sent with 2:1 compression"
            checked={params.settings.compander ?? false}
            onChange={(compander) => set({ ...params.settings, compander })}
          />
        </>
      );
    }
    case "selcall":
      return (
        <SettingRow label="Tone plan">
          <Segmented
            label="Selective calling tone plan"
            value={params.settings.system ?? "ccir1"}
            options={SELCALL_SYSTEMS}
            onChange={(system) =>
              onParams({ type: "selcall", settings: { ...params.settings, system } })
            }
          />
        </SettingRow>
      );
    case "am":
      return (
        <>
          <SettingRow label="Bandwidth">
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 10_000}
              optionsHz={[5_000, 8_000, 10_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "am", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </SettingRow>
        </>
      );
    case "ssb":
      return (
        <>
          <SettingRow label="Sideband">
            <Segmented
              label="Sideband"
              value={params.settings.sideband ?? "usb"}
              options={SIDEBANDS}
              onChange={(sideband) =>
                onParams({ type: "ssb", settings: { ...params.settings, sideband } })
              }
            />
          </SettingRow>
          <SettingRow label="Bandwidth">
            <NumberField
              label="SSB bandwidth (Hz)"
              value={params.settings.bandwidth_hz ?? 2_700}
              {...limitOf(limits, "bandwidth_hz")}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "ssb", settings: { ...params.settings, bandwidth_hz } })
              }
              className="w-20"
            />
            <span className="legend">Hz</span>
          </SettingRow>
        </>
      );
    case "wfm":
      return (
        <>
          <SettingRow label="De-emphasis">
            <Select
              label="De-emphasis (µs)"
              value={params.settings.deemphasis_us ?? 50}
              options={DEEMPHASIS_US}
              onChange={(deemphasis_us) =>
                onParams({ type: "wfm", settings: { ...params.settings, deemphasis_us } })
              }
            />
          </SettingRow>
          <Toggle
            label="Stereo"
            title="Decode the stereo pilot; mono is quieter on weak signals"
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
          <SettingRow label="Baud">
            <Select
              label="POCSAG baud"
              value={params.settings.baud ?? "auto"}
              options={POCSAG_BAUDS}
              onChange={(baud) =>
                onParams({ type: "pocsag", settings: { ...params.settings, baud } })
              }
            />
          </SettingRow>
          <SettingRow label="Bandwidth">
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 12_500}
              optionsHz={[12_500, 25_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "pocsag", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </SettingRow>
          <Toggle
            label="Invert"
            title="Flip the signal's polarity; try it when nothing decodes"
            checked={params.settings.invert ?? false}
            onChange={(invert) =>
              onParams({ type: "pocsag", settings: { ...params.settings, invert } })
            }
          />
        </>
      );
    case "flex":
    case "ermes": {
      const type = params.type;
      const label = type === "flex" ? "FLEX" : "ERMES";
      return (
        <>
          <SettingRow label="Bandwidth">
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 12_500}
              optionsHz={[12_500, 25_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type, settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </SettingRow>
          <Toggle
            label={`Invert ${label}`}
            title="Flip the signal's polarity; try it when nothing decodes"
            checked={params.settings.invert ?? false}
            onChange={(invert) => onParams({ type, settings: { ...params.settings, invert } })}
          />
        </>
      );
    }
    case "adsb":
      return (
        <Toggle
          label="CRC fix"
          title="Repair single-bit errors the checksum can pin down"
          checked={params.settings.crc_fix ?? true}
          onChange={(crc_fix) =>
            onParams({ type: "adsb", settings: { ...params.settings, crc_fix } })
          }
        />
      );
    case "ais":
      return (
        <SettingRow label="Channel">
          <Segmented
            label="AIS channel"
            value={params.settings.ais_channel ?? "a"}
            options={AIS_CHANNELS}
            onChange={(ais_channel) =>
              onParams({ type: "ais", settings: { ...params.settings, ais_channel } })
            }
          />
        </SettingRow>
      );
    case "aprs":
      return (
        <>
          <SettingRow label="Mode">
            <Select
              label="APRS mode"
              value={params.settings.mode ?? "afsk1200"}
              options={APRS_MODES}
              onChange={(mode) =>
                onParams({ type: "aprs", settings: { ...params.settings, mode } })
              }
            />
          </SettingRow>
          <SettingRow label="Bandwidth">
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 12_500}
              optionsHz={[12_500, 25_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "aprs", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </SettingRow>
        </>
      );
    case "rtty":
      return (
        <>
          <SettingRow label="Baud">
            <PresetNumberField
              label="RTTY baud"
              value={params.settings.baud ?? 45.45}
              presets={RTTY_BAUDS}
              limit={limitOf(limits, "baud")}
              onCommit={(baud) =>
                onParams({ type: "rtty", settings: { ...params.settings, baud } })
              }
            />
          </SettingRow>
          <SettingRow label="Shift">
            <PresetNumberField
              label="RTTY shift (Hz)"
              value={params.settings.shift_hz ?? 170}
              presets={RTTY_SHIFTS_HZ}
              limit={limitOf(limits, "shift_hz")}
              onCommit={(shift_hz) =>
                onParams({ type: "rtty", settings: { ...params.settings, shift_hz } })
              }
            />
            <span className="legend">Hz</span>
          </SettingRow>
          <SettingRow label="Stop bits">
            <Select
              label="RTTY stop bits"
              value={params.settings.stop_bits ?? "one_and_half"}
              options={RTTY_STOP_BITS}
              onChange={(stop_bits) =>
                onParams({ type: "rtty", settings: { ...params.settings, stop_bits } })
              }
            />
          </SettingRow>
          <Toggle
            label="Invert"
            title="Flip the signal's polarity; try it when nothing decodes"
            checked={params.settings.invert ?? false}
            onChange={(invert) =>
              onParams({ type: "rtty", settings: { ...params.settings, invert } })
            }
          />
          <Toggle
            label="Unshift on space"
            title="Drop back to letters after a space, as most stations expect"
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
          <SettingRow label="Bandwidth">
            <NumberField
              label="CW filter bandwidth (Hz)"
              value={params.settings.bandwidth_hz ?? 400}
              {...limitOf(limits, "bandwidth_hz")}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "morse", settings: { ...params.settings, bandwidth_hz } })
              }
            />
            <span className="legend">Hz</span>
          </SettingRow>
          <SettingRow label="WPM">
            <OptionalNumberField
              label="Morse speed (WPM), empty to auto-track"
              placeholder="auto"
              value={params.settings.wpm ?? null}
              {...limitOf(limits, "wpm")}
              onCommit={(wpm) => onParams({ type: "morse", settings: { ...params.settings, wpm } })}
            />
          </SettingRow>
        </>
      );
    case "cw_skimmer":
      return (
        <>
          <SettingRow label="Passband">
            <NumberField
              label="CW skimmer passband (Hz)"
              value={params.settings.bandwidth_hz ?? 24_000}
              {...limitOf(limits, "bandwidth_hz")}
              onCommit={(bandwidth_hz) =>
                onParams({
                  type: "cw_skimmer",
                  settings: { ...params.settings, bandwidth_hz },
                })
              }
              className="w-24"
            />
            <span className="legend">Hz</span>
          </SettingRow>
          <SettingRow label="Acquire">
            <NumberField
              label="Carrier threshold above the noise floor (dB)"
              value={params.settings.threshold_db ?? 10}
              {...limitOf(limits, "threshold_db")}
              onCommit={(threshold_db) =>
                onParams({
                  type: "cw_skimmer",
                  settings: { ...params.settings, threshold_db },
                })
              }
              className="w-16"
            />
            <span className="legend">dB SNR</span>
          </SettingRow>
          <SettingRow label="Signals">
            <NumberField
              label="Maximum simultaneous CW signals"
              value={params.settings.max_signals ?? 32}
              {...limitOf(limits, "max_signals")}
              onCommit={(max_signals) =>
                onParams({
                  type: "cw_skimmer",
                  settings: { ...params.settings, max_signals },
                })
              }
              className="w-16"
            />
          </SettingRow>
          <SettingRow label="WPM">
            <OptionalNumberField
              label="Morse speed (WPM), empty to track each signal"
              placeholder="auto"
              value={params.settings.wpm ?? null}
              {...limitOf(limits, "wpm")}
              onCommit={(wpm) =>
                onParams({ type: "cw_skimmer", settings: { ...params.settings, wpm } })
              }
            />
          </SettingRow>
        </>
      );
    case "ft8":
      return (
        <WsjtControls
          mode="ft8"
          limits={limits}
          settings={params.settings}
          onChange={(settings) => onParams({ type: "ft8", settings })}
        />
      );
    case "ft4":
      return (
        <WsjtControls
          mode="ft4"
          limits={limits}
          settings={params.settings}
          onChange={(settings) => onParams({ type: "ft4", settings })}
        />
      );
    case "wspr":
      return (
        <WsjtControls
          mode="wspr"
          limits={limits}
          settings={params.settings}
          onChange={(settings) => onParams({ type: "wspr", settings })}
        />
      );
    case "psk":
      return (
        <>
          <SettingRow label="Mode">
            <Select
              label="PSK symbol rate"
              value={params.settings.baud ?? "psk31"}
              options={PSK_BAUDS}
              onChange={(baud) => onParams({ type: "psk", settings: { ...params.settings, baud } })}
            />
          </SettingRow>
          <Toggle
            label="Invert"
            title="Flip the signal's polarity; try it when nothing decodes"
            checked={params.settings.invert ?? false}
            onChange={(invert) =>
              onParams({ type: "psk", settings: { ...params.settings, invert } })
            }
          />
        </>
      );
    case "navtex":
      return (
        <Toggle
          label="Invert"
          title="Flip the signal's polarity; try it when nothing decodes"
          checked={params.settings.invert ?? false}
          onChange={(invert) =>
            onParams({ type: "navtex", settings: { ...params.settings, invert } })
          }
        />
      );
    case "radio_clock":
      return (
        <>
          <SettingRow label="Service">
            <Select
              label="Radio clock service"
              value={params.settings.standard ?? "dcf77"}
              options={RADIO_CLOCK_STANDARDS}
              onChange={(standard) =>
                onParams({
                  type: "radio_clock",
                  settings: { ...params.settings, standard },
                })
              }
            />
          </SettingRow>
          <Toggle
            label="Invert"
            title="Flip the signal's polarity; try it when nothing decodes"
            checked={params.settings.invert ?? false}
            onChange={(invert) =>
              onParams({ type: "radio_clock", settings: { ...params.settings, invert } })
            }
          />
        </>
      );
    case "gnss":
      return (
        <>
          <SettingRow label="GPS PRN">
            <NumberField
              label="GPS L1 C/A satellite PRN"
              value={params.settings.prn ?? 1}
              {...limitOf(limits, "prn")}
              onCommit={(prn) => onParams({ type: "gnss", settings: { ...params.settings, prn } })}
              className="w-16"
            />
          </SettingRow>
          <SettingRow label="Doppler">
            <NumberField
              label="Symmetric Doppler search span (Hz)"
              value={params.settings.doppler_hz ?? 10_000}
              {...limitOf(limits, "doppler_hz")}
              onCommit={(doppler_hz) =>
                onParams({ type: "gnss", settings: { ...params.settings, doppler_hz } })
              }
              className="w-20"
            />
            <span className="legend">Hz</span>
          </SettingRow>
          <SettingRow label="Acquire above">
            <NumberField
              label="Correlation peak-to-floor acquisition threshold"
              value={params.settings.threshold ?? 2.5}
              {...limitOf(limits, "threshold")}
              onCommit={(threshold) =>
                onParams({ type: "gnss", settings: { ...params.settings, threshold } })
              }
              className="w-16"
            />
            <span className="legend">× floor</span>
          </SettingRow>
        </>
      );
    case "vor": {
      const set = (settings: ChannelParamsOf<"vor">) => onParams({ type: "vor", settings });
      return (
        <>
          <SettingRow label="Station">
            <TextAutocomplete
              label="VOR station identifier"
              value={params.settings.station ?? ""}
              suggestions={[]}
              placeholder="Optional identifier"
              onCommit={(station) => {
                set({ ...params.settings, station: station === "" ? undefined : station });
                return true;
              }}
            />
          </SettingRow>
          <SettingRow label="Station latitude">
            <OptionalNumberField
              label="VOR station latitude"
              placeholder="Unknown"
              value={params.settings.station_lat ?? null}
              {...limitOf(limits, "station_lat")}
              onCommit={(station_lat) => set({ ...params.settings, station_lat })}
            />
          </SettingRow>
          <SettingRow label="Station longitude">
            <OptionalNumberField
              label="VOR station longitude"
              placeholder="Unknown"
              value={params.settings.station_lon ?? null}
              {...limitOf(limits, "station_lon")}
              onCommit={(station_lon) => set({ ...params.settings, station_lon })}
            />
          </SettingRow>
          <SettingRow label="Declination">
            <NumberField
              label="East-positive magnetic declination at the VOR"
              value={params.settings.magnetic_declination_deg ?? 0}
              {...limitOf(limits, "magnetic_declination_deg")}
              onCommit={(magnetic_declination_deg) =>
                set({ ...params.settings, magnetic_declination_deg })
              }
            />
            <span className="legend">°</span>
          </SettingRow>
          <SettingRow label="Report every">
            <NumberField
              label="VOR report interval in milliseconds"
              value={params.settings.report_ms ?? 500}
              {...limitOf(limits, "report_ms")}
              onCommit={(report_ms) => set({ ...params.settings, report_ms })}
            />
            <span className="legend">ms</span>
          </SettingRow>
        </>
      );
    }
    case "ils":
      return (
        <>
          <SettingRow label="Component">
            <Segmented
              label="ILS component"
              value={params.settings.component ?? "localizer"}
              options={ILS_COMPONENTS}
              onChange={(component) =>
                onParams({ type: "ils", settings: { ...params.settings, component } })
              }
            />
          </SettingRow>
          <SettingRow label="Report every">
            <NumberField
              label="ILS report interval in milliseconds"
              value={params.settings.report_ms ?? 500}
              {...limitOf(limits, "report_ms")}
              onCommit={(report_ms) =>
                onParams({ type: "ils", settings: { ...params.settings, report_ms } })
              }
            />
            <span className="legend">ms</span>
          </SettingRow>
        </>
      );
    case "acars":
      return (
        <SettingRow label="Bandwidth">
          <BandwidthSelect
            valueHz={params.settings.bandwidth_hz ?? 12_500}
            optionsHz={[8_000, 12_500, 25_000]}
            onCommit={(bandwidth_hz) =>
              onParams({ type: "acars", settings: { ...params.settings, bandwidth_hz } })
            }
          />
        </SettingRow>
      );
    case "subghz":
      return (
        <>
          <SettingRow label="Modulation">
            <Segmented
              label="Modulation"
              value={params.settings.modulation ?? "ook"}
              options={SUBGHZ_MODULATIONS}
              onChange={(modulation) =>
                onParams({ type: "subghz", settings: { ...params.settings, modulation } })
              }
            />
          </SettingRow>
          <SettingRow label="Bandwidth">
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 150_000}
              optionsHz={[50_000, 100_000, 150_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "subghz", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </SettingRow>
          <SettingRow label="Min pulse">
            <NumberField
              label="Shortest keying edge accepted (µs)"
              value={params.settings.min_pulse_us ?? 80}
              {...limitOf(limits, "min_pulse_us")}
              onCommit={(min_pulse_us) =>
                onParams({ type: "subghz", settings: { ...params.settings, min_pulse_us } })
              }
              className="w-20"
            />
            <span className="legend">µs</span>
          </SettingRow>
          <SettingRow label="Frame gap">
            <NumberField
              label="Silence that ends a frame (µs)"
              value={params.settings.frame_gap_us ?? 5_000}
              {...limitOf(limits, "frame_gap_us")}
              onCommit={(frame_gap_us) =>
                onParams({ type: "subghz", settings: { ...params.settings, frame_gap_us } })
              }
              className="w-24"
            />
            <span className="legend">µs</span>
          </SettingRow>
        </>
      );
    case "atv":
      return (
        <>
          <SettingRow label="Modulation">
            <Segmented
              label="Modulation"
              value={params.settings.modulation ?? "am"}
              options={ATV_MODULATIONS}
              onChange={(modulation) =>
                onParams({ type: "atv", settings: { ...params.settings, modulation } })
              }
            />
          </SettingRow>
          <SettingRow label="Lines">
            <Select
              label="Scanning standard"
              value={params.settings.standard ?? "ccir625"}
              options={ATV_STANDARDS}
              onChange={(standard) =>
                onParams({ type: "atv", settings: { ...params.settings, standard } })
              }
            />
          </SettingRow>
          <SettingRow label="Bandwidth">
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 1_500_000}
              optionsHz={[500_000, 1_000_000, 1_500_000, 1_600_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "atv", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </SettingRow>
          <SettingRow label="Colour">
            <Select
              label="Composite colour system"
              value={params.settings.color ?? "monochrome"}
              options={ATV_COLORS}
              onChange={(color) =>
                onParams({ type: "atv", settings: { ...params.settings, color } })
              }
            />
          </SettingRow>
          <SettingRow label="Sound">
            <OptionalNumberField
              label="FM sound subcarrier (MHz), empty for none"
              placeholder="off"
              value={
                params.settings.sound_subcarrier_hz == null
                  ? null
                  : params.settings.sound_subcarrier_hz / 1_000_000
              }
              {...scaledLimit(limitOf(limits, "sound_subcarrier_hz"), 1e-6)}
              onCommit={(mhz) =>
                onParams({
                  type: "atv",
                  settings: {
                    ...params.settings,
                    sound_subcarrier_hz: mhz === null ? null : mhz * 1_000_000,
                  },
                })
              }
            />
            <span className="legend">MHz</span>
          </SettingRow>
          <Toggle
            label="Interlace"
            title="Weave both fields into one frame"
            checked={params.settings.interlace ?? true}
            onChange={(interlace) =>
              onParams({ type: "atv", settings: { ...params.settings, interlace } })
            }
          />
          <Toggle
            label="Invert"
            title="Flip the signal's polarity; try it when nothing decodes"
            checked={params.settings.invert ?? false}
            onChange={(invert) =>
              onParams({ type: "atv", settings: { ...params.settings, invert } })
            }
          />
        </>
      );
    case "sstv":
      return (
        <>
          <SettingRow label="Mode">
            <Select
              label="Scanning mode"
              value={params.settings.mode ?? SSTV_AUTO}
              options={SSTV_MODES}
              onChange={(mode) =>
                onParams({
                  type: "sstv",
                  settings: {
                    ...params.settings,
                    mode: mode === SSTV_AUTO ? null : mode,
                  },
                })
              }
            />
          </SettingRow>
          <Toggle
            label="Slant correction"
            title="Straighten pictures from a sender whose clock runs off"
            checked={params.settings.slant_correction ?? true}
            onChange={(slant_correction) =>
              onParams({ type: "sstv", settings: { ...params.settings, slant_correction } })
            }
          />
          <Toggle
            label="Keep unfinished pictures"
            title="Keep a picture even when the transmission stops early"
            checked={params.settings.keep_partial ?? true}
            onChange={(keep_partial) =>
              onParams({ type: "sstv", settings: { ...params.settings, keep_partial } })
            }
          />
        </>
      );
    case "dab":
      return (
        <>
          <SettingRow label="Generation">
            <Segmented
              label="DAB generation"
              value={params.settings.mode ?? "auto"}
              options={DAB_MODES}
              onChange={(mode) => onParams({ type: "dab", settings: { ...params.settings, mode } })}
            />
          </SettingRow>
          <SettingRow label="Transmission">
            <Segmented
              label="DAB transmission mode"
              value={params.settings.transmission_mode ?? "i"}
              options={DAB_TRANSMISSION_MODES}
              onChange={(transmission_mode) =>
                onParams({ type: "dab", settings: { ...params.settings, transmission_mode } })
              }
            />
          </SettingRow>
          <BroadcastServicePicker
            status={broadcast}
            value={params.settings.service_id ?? null}
            max={0xffffffff}
            onChange={(service_id) =>
              onParams({ type: "dab", settings: { ...params.settings, service_id } })
            }
          />
        </>
      );
    case "datv":
      return (
        <>
          <SettingRow label="Standard">
            <Segmented
              label="DATV standard"
              value={params.settings.standard ?? "dvb_s"}
              options={DATV_STANDARDS}
              onChange={(standard) =>
                onParams({ type: "datv", settings: { ...params.settings, standard } })
              }
            />
          </SettingRow>
          <SettingRow label="Symbol rate">
            <NumberField
              label="DATV symbol rate (baud)"
              value={params.settings.symbol_rate ?? 333_000}
              {...limitOf(limits, "symbol_rate")}
              onCommit={(symbol_rate) =>
                onParams({ type: "datv", settings: { ...params.settings, symbol_rate } })
              }
              className="w-28"
            />
            <span className="legend">Bd</span>
          </SettingRow>
          <BroadcastServicePicker
            status={broadcast}
            value={params.settings.program ?? null}
            max={65535}
            onChange={(program) =>
              onParams({ type: "datv", settings: { ...params.settings, program } })
            }
          />
          {params.settings.standard === "dvb_s2" ? (
            <>
              <Toggle
                label="Superframes"
                title="Receive Annex E format 0 or 1 with the default reference and payload scrambling codes"
                checked={params.settings.superframes ?? false}
                onChange={(superframes) =>
                  onParams({ type: "datv", settings: { ...params.settings, superframes } })
                }
              />
              <SettingRow
                label="Input stream"
                title="Choose an input stream identifier on a multistream carrier"
              >
                <OptionalNumberField
                  label="DVB-S2 input stream"
                  placeholder="Auto"
                  value={params.settings.input_stream ?? null}
                  min={0}
                  max={255}
                  step={1}
                  onCommit={(input_stream) =>
                    onParams({ type: "datv", settings: { ...params.settings, input_stream } })
                  }
                />
              </SettingRow>
            </>
          ) : (
            <SettingRow label="Code rate">
              <Select
                label="DVB-S code rate"
                value={params.settings.code_rate ?? "auto"}
                options={[
                  { value: "auto", label: "Auto" },
                  { value: "half", label: "1/2" },
                  { value: "two_thirds", label: "2/3" },
                  { value: "three_quarters", label: "3/4" },
                  { value: "five_sixths", label: "5/6" },
                  { value: "seven_eighths", label: "7/8" },
                ]}
                onChange={(code_rate) =>
                  onParams({ type: "datv", settings: { ...params.settings, code_rate } })
                }
              />
            </SettingRow>
          )}
        </>
      );
    case "dvbt":
      return (
        <>
          <SettingRow label="Bandwidth">
            <Segmented
              label="DVB-T bandwidth"
              value={params.settings.bandwidth ?? "mhz8"}
              options={[
                { value: "mhz6", label: "6 MHz" },
                { value: "mhz7", label: "7 MHz" },
                { value: "mhz8", label: "8 MHz" },
              ]}
              onChange={(bandwidth) =>
                onParams({ type: "dvbt", settings: { ...params.settings, bandwidth } })
              }
            />
          </SettingRow>
          <Toggle
            label="Low priority stream"
            title="Decode the low priority transport stream of a hierarchical DVB-T multiplex"
            checked={params.settings.low_priority ?? false}
            onChange={(low_priority) =>
              onParams({ type: "dvbt", settings: { ...params.settings, low_priority } })
            }
          />
          <BroadcastServicePicker
            status={broadcast}
            value={params.settings.program ?? null}
            max={65535}
            onChange={(program) =>
              onParams({ type: "dvbt", settings: { ...params.settings, program } })
            }
          />
        </>
      );
    case "drm": {
      const mode = params.settings.mode ?? "auto";
      return (
        <>
          <SettingRow label="Mode">
            <Segmented
              label="DRM mode"
              value={mode}
              options={DRM_MODES}
              onChange={(nextMode) =>
                onParams({
                  type: "drm",
                  settings: {
                    ...params.settings,
                    mode: nextMode,
                    bandwidth_hz:
                      nextMode === "drm30"
                        ? mode === "drm30"
                          ? params.settings.bandwidth_hz
                          : 10_000
                        : 100_000,
                  },
                })
              }
            />
          </SettingRow>
          <SettingRow label="Bandwidth">
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 100_000}
              optionsHz={
                mode === "drm30" ? [4_500, 5_000, 9_000, 10_000, 18_000, 20_000] : [100_000]
              }
              onCommit={(bandwidth_hz) =>
                onParams({ type: "drm", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </SettingRow>
        </>
      );
    }
    case "dmr":
      return (
        <>
          <SettingRow label="Slot">
            <Segmented
              label="Slot"
              value={params.settings.slots ?? "both"}
              options={DMR_SLOTS}
              onChange={(slots) =>
                onParams({ type: "dmr", settings: { ...params.settings, slots } })
              }
            />
          </SettingRow>
          <Toggle
            label="Ignore data CRC"
            title="Show data blocks whose checksum fails"
            checked={params.settings.ignore_crc ?? false}
            onChange={(ignore_crc) =>
              onParams({ type: "dmr", settings: { ...params.settings, ignore_crc } })
            }
          />
        </>
      );
    case "nxdn":
      return (
        <SettingRow label="Width">
          <Segmented
            label="Width"
            value={params.settings.bandwidth ?? "narrow"}
            options={NXDN_WIDTHS}
            onChange={(bandwidth) =>
              onParams({ type: "nxdn", settings: { ...params.settings, bandwidth } })
            }
          />
        </SettingRow>
      );
    case "freedv":
      return (
        <SettingRow label="Sideband">
          <Segmented
            label="FreeDV sideband"
            value={params.settings.sideband ?? "usb"}
            options={SIDEBANDS}
            onChange={(sideband) =>
              onParams({ type: "freedv", settings: { ...params.settings, sideband } })
            }
          />
        </SettingRow>
      );
    case "ident":
      return (
        <>
          <SettingRow label="Search width">
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 192_000}
              optionsHz={[12_500, 50_000, 100_000, 192_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "ident", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </SettingRow>
          <SettingRow label="Report every">
            <NumberField
              label="Milliseconds of signal each report is measured from"
              value={params.settings.interval_ms ?? 1_000}
              {...limitOf(limits, "interval_ms")}
              onCommit={(interval_ms) =>
                onParams({ type: "ident", settings: { ...params.settings, interval_ms } })
              }
              className="w-20"
            />
            <span className="legend">ms</span>
          </SettingRow>
          <SettingRow label="Detect above">
            <NumberField
              label="Decibels above the noise floor a signal must reach"
              value={params.settings.threshold_db ?? 8}
              {...limitOf(limits, "threshold_db")}
              onCommit={(threshold_db) =>
                onParams({ type: "ident", settings: { ...params.settings, threshold_db } })
              }
              className="w-16"
            />
            <span className="legend">dB</span>
          </SettingRow>
        </>
      );
    case "dect":
      return (
        <>
          <SettingRow label="Band">
            <Segmented
              label="DECT band"
              value={params.settings.band ?? "eu"}
              options={DECT_BANDS}
              onChange={(band) =>
                onParams({ type: "dect", settings: { ...params.settings, band } })
              }
            />
          </SettingRow>
          <SettingRow label="Side">
            <Segmented
              label="DECT side"
              value={params.settings.sides ?? "both"}
              options={DECT_SIDES}
              onChange={(sides) =>
                onParams({ type: "dect", settings: { ...params.settings, sides } })
              }
            />
          </SettingRow>
        </>
      );
    case "dstar":
    case "ysf":
    case "p25":
    case "dpmr":
    case "m17":
    case "dsc":
    case "inmarsat_stdc":
    case "inmarsat_aero":
    case "vdl2":
    case "hfdl":
    case "iridium":
      return null;
    default:
      return unhandledMode(params);
  }
}

function WsjtControls({
  mode,
  limits,
  settings,
  onChange,
}: {
  mode: "ft8" | "ft4" | "wspr";
  limits: readonly ParamLimit[];
  settings: ChannelParamsOf<"ft8">;
  onChange: (settings: ChannelParamsOf<"ft8">) => void;
}) {
  const wspr = mode === "wspr";
  return (
    <>
      <SettingRow label="Audio from">
        <NumberField
          label="Lowest USB audio frequency searched"
          value={settings.audio_low_hz ?? (wspr ? 1_400 : 200)}
          {...limitOf(limits, "audio_low_hz")}
          onCommit={(audio_low_hz) => onChange({ ...settings, audio_low_hz })}
        />
        <span className="legend">Hz</span>
      </SettingRow>
      <SettingRow label="Audio to">
        <NumberField
          label="Highest USB audio frequency searched"
          value={settings.audio_high_hz ?? (wspr ? 1_600 : 3_000)}
          {...limitOf(limits, "audio_high_hz")}
          onCommit={(audio_high_hz) => onChange({ ...settings, audio_high_hz })}
        />
        <span className="legend">Hz</span>
      </SettingRow>
      <SettingRow label="Candidates">
        <NumberField
          label="Maximum synchronized signals tried per decode pass"
          value={settings.max_candidates ?? 200}
          {...limitOf(limits, "max_candidates")}
          onCommit={(max_candidates) => onChange({ ...settings, max_candidates })}
        />
      </SettingRow>
    </>
  );
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
    optionsHz.map((hz) => ({ value: hz, label: formatHz(hz) })),
    formatHz,
  );
  return <Select label="Channel bandwidth" value={valueHz} options={options} onChange={onCommit} />;
}

function Toggle({
  label,
  title,
  checked,
  onChange,
}: {
  label: string;
  title?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <SettingRow label={label} title={title}>
      <Checkbox label={label} checked={checked} onChange={onChange} />
    </SettingRow>
  );
}

function PresetNumberField({
  label,
  value,
  presets,
  limit,
  onCommit,
}: {
  label: string;
  value: number;
  presets: Options<number>;
  limit: NumberLimit;
  onCommit: (value: number) => void;
}) {
  return (
    <>
      <Segmented label={`${label} presets`} value={value} options={presets} onChange={onCommit} />
      <NumberField label={label} value={value} {...limit} onCommit={onCommit} />
    </>
  );
}

function BroadcastServicePicker({
  status,
  value,
  max,
  onChange,
}: {
  status?: BroadcastStatus;
  value: number | null;
  max: number;
  onChange: (id: number | null) => void;
}) {
  const services = status?.services ?? [];
  const options = [
    { value: "", label: "Auto" },
    ...services.map((service) => ({
      value: String(service.id),
      label: service.label || String(service.id),
    })),
  ];
  if (value !== null && !services.some((service) => service.id === value))
    options.push({ value: String(value), label: String(value) });
  return (
    <SettingRow
      label="Service"
      title="Select a discovered audio, video or data service; Auto chooses the first playable service"
    >
      {services.length > 0 ? (
        <Select
          label="Broadcast service"
          value={value === null ? "" : String(value)}
          options={options}
          onChange={(id) => onChange(id === "" ? null : Number(id))}
        />
      ) : (
        <OptionalNumberField
          label="Broadcast service identifier"
          placeholder="Auto"
          value={value}
          min={0}
          max={max}
          step={1}
          onCommit={onChange}
        />
      )}
    </SettingRow>
  );
}
