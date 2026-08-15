import { useState } from "react";
import type { ChannelDescriptor, ChannelInfo, ChannelParams } from "../lib/types";
import { type ChannelEdit, useChannelPatch } from "../lib/useChannelPatch";
import { Checkbox } from "./Checkbox";
import { type ChannelParamsOf, offsetLimitHz } from "./channelSettings";
import type { Options } from "./controls";
import { formatKhz } from "./format";
import { NumberField, OptionalNumberField } from "./NumberField";
import { OffsetStepper } from "./OffsetStepper";
import { Segmented } from "./Segmented";
import { Select } from "./Select";
import { SettingRow, Settings } from "./Settings";
import { Slider } from "./Slider";
import { withCurrent } from "./selectOptions";
import { useDebouncedCommit } from "./useDebouncedCommit";

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

  return (
    <Settings className="p-2">
      <SettingRow label="Offset (kHz)">
        <OffsetStepper
          offsetHz={offsetHz}
          limitHz={limitHz}
          onOffset={(offset_hz) => onEdit({ offset_hz })}
        />
      </SettingRow>

      <SettingRow label="Squelch">
        <Checkbox
          label="Squelch"
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
        {/* Drawn whether or not squelch is on, so switching it does not resize the face under
            the pointer — off, the threshold is the one it will open at. */}
        <Slider
          label="Squelch threshold (dB)"
          className="min-w-0 flex-1"
          disabled={squelchDb === null}
          min={-120}
          max={0}
          step={1}
          value={squelchSlider.pending ?? squelchDb ?? offSquelchDb}
          onChange={squelchSlider.change}
        />
        <span
          className={`w-14 shrink-0 text-right font-mono text-xs tabular-nums ${
            squelchDb === null ? "text-ink-faint opacity-45" : "text-ink"
          }`}
        >
          {(squelchSlider.pending ?? squelchDb ?? offSquelchDb).toFixed(0)}{" "}
          <span className="text-ink-faint">dB</span>
        </span>
      </SettingRow>

      <ModeControls params={settings.params} onParams={(params) => onEdit({ params })} />
    </Settings>
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
        </>
      );
    }
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
              min={200}
              max={10_000}
              step={100}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "ssb", settings: { ...params.settings, bandwidth_hz } })
              }
              className="w-20"
            />
            <span className="legend">Hz</span>
          </SettingRow>
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
            checked={params.settings.invert ?? false}
            onChange={(invert) =>
              onParams({ type: "pocsag", settings: { ...params.settings, invert } })
            }
          />
        </>
      );
    case "adsb":
      return (
        <Toggle
          label="CRC fix"
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
              min={10}
              max={1_200}
              step={0.05}
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
              min={20}
              max={2_000}
              step={5}
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
          <SettingRow label="Bandwidth">
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
            <span className="legend">Hz</span>
          </SettingRow>
          <SettingRow label="WPM">
            <OptionalNumberField
              label="Morse speed (WPM), empty to auto-track"
              placeholder="auto"
              value={params.settings.wpm ?? null}
              min={5}
              max={60}
              step={1}
              onCommit={(wpm) => onParams({ type: "morse", settings: { ...params.settings, wpm } })}
            />
          </SettingRow>
        </>
      );
    case "ft8":
      return (
        <WsjtControls
          mode="ft8"
          settings={params.settings}
          onChange={(settings) => onParams({ type: "ft8", settings })}
        />
      );
    case "ft4":
      return (
        <WsjtControls
          mode="ft4"
          settings={params.settings}
          onChange={(settings) => onParams({ type: "ft4", settings })}
        />
      );
    case "wspr":
      return (
        <WsjtControls
          mode="wspr"
          settings={params.settings}
          onChange={(settings) => onParams({ type: "wspr", settings })}
        />
      );
    case "psk31":
      return (
        <Toggle
          label="Invert"
          checked={params.settings.invert ?? false}
          onChange={(invert) =>
            onParams({ type: "psk31", settings: { ...params.settings, invert } })
          }
        />
      );
    case "psk63":
      return (
        <Toggle
          label="Invert"
          checked={params.settings.invert ?? false}
          onChange={(invert) =>
            onParams({ type: "psk63", settings: { ...params.settings, invert } })
          }
        />
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
              min={10}
              max={2_000}
              step={10}
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
              min={500}
              max={100_000}
              step={500}
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
              min={250}
              max={10_000}
              step={250}
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
              min={3}
              max={40}
              step={1}
              onCommit={(threshold_db) =>
                onParams({ type: "ident", settings: { ...params.settings, threshold_db } })
              }
              className="w-16"
            />
            <span className="legend">dB</span>
          </SettingRow>
        </>
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

function WsjtControls({
  mode,
  settings,
  onChange,
}: {
  mode: "ft8" | "ft4" | "wspr";
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
          min={50}
          max={5_450}
          step={10}
          onCommit={(audio_low_hz) => onChange({ ...settings, audio_low_hz })}
        />
        <span className="legend">Hz</span>
      </SettingRow>
      <SettingRow label="Audio to">
        <NumberField
          label="Highest USB audio frequency searched"
          value={settings.audio_high_hz ?? (wspr ? 1_600 : 3_000)}
          min={100}
          max={5_500}
          step={10}
          onCommit={(audio_high_hz) => onChange({ ...settings, audio_high_hz })}
        />
        <span className="legend">Hz</span>
      </SettingRow>
      <SettingRow label="Candidates">
        <NumberField
          label="Maximum synchronized signals tried per decode pass"
          value={settings.max_candidates ?? (wspr ? 200 : 50)}
          min={1}
          max={1_000}
          step={1}
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
    <SettingRow label={label}>
      <Checkbox label={label} checked={checked} onChange={onChange} />
    </SettingRow>
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
    <>
      <Segmented label={`${label} presets`} value={value} options={presets} onChange={onCommit} />
      <NumberField
        label={label}
        value={value}
        min={min}
        max={max}
        step={step}
        onCommit={onCommit}
      />
    </>
  );
}
