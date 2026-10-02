import type { ChannelParams, DecoderEvent, ParamLimit } from "../lib/types";
import { type ChannelTypeId, limitOf, type NumberLimit, scaledLimit } from "./channelSettings";
import type { Options } from "./controls";
import {
  ChipField,
  ChoiceChip,
  NumberChip,
  OptionalNumberChip,
  SettingChip,
  ToggleChip,
} from "./face/Chips";
import { formatHz } from "./format";
import {
  AERO_CHANNELS,
  AIS_CHANNELS,
  APRS_MODES,
  ATV_COLORS,
  ATV_MODULATIONS,
  ATV_STANDARDS,
  CTCSS_DEFAULT_HZ,
  CTCSS_OPTIONS,
  DAB_MODES,
  DAB_TRANSMISSION_MODES,
  DATV_CODE_RATES,
  DATV_ROLL_OFFS,
  DATV_STANDARDS,
  DCS_DEFAULT_CODE,
  DCS_OPTIONS,
  DECT_BANDS,
  DECT_SIDES,
  DEEMPHASIS_US,
  DMR_SLOTS,
  DRM_MODES,
  DVBT_BANDWIDTHS,
  DVBT_STANDARDS,
  ILS_COMPONENTS,
  INVERSION_DEFAULT_HZ,
  IRIDIUM_SPANS,
  LRPT_MODES,
  NFM_SCRAMBLER_MODES,
  NFM_TONE_MODES,
  NXDN_WIDTHS,
  POCSAG_BAUDS,
  PSK_BAUDS,
  RADIO_CLOCK_STANDARDS,
  RTTY_BAUDS,
  RTTY_SHIFTS_HZ,
  RTTY_STOP_BITS,
  SELCALL_SYSTEMS,
  SIDEBANDS,
  SONDE_AUTO,
  SONDE_TYPES,
  SSTV_AUTO,
  SSTV_MODES,
  WEFAX_IOCS,
  WEFAX_LPMS,
} from "./modeOptions";
import { NumberField } from "./NumberField";
import { Segmented } from "./Segmented";
import { withCurrent } from "./selectOptions";
import { TextAutocomplete } from "./TextAutocomplete";

export type BroadcastStatus = Extract<DecoderEvent, { kind: "broadcast" }>["data"];

type Limits = readonly ParamLimit[];
type ParamsOf<K extends ChannelTypeId> = Extract<ChannelParams, { type: K }>;

interface Mode<K extends ChannelTypeId> {
  params: ParamsOf<K>;
  limits: Limits;
  onParams: (params: ChannelParams) => void;
}

const INVERT_TITLE = "Flip the signal's polarity; try it when nothing decodes";
const SERVICE_TITLE =
  "Select a discovered audio, video or data service; Auto chooses the first playable service";

export function ModeChips({
  params,
  broadcast,
  limits,
  onParams,
}: {
  params: ChannelParams;
  broadcast?: BroadcastStatus;
  limits: Limits;
  onParams: (params: ChannelParams) => void;
}) {
  const mode = { limits, onParams };
  switch (params.type) {
    case "nfm":
      return <NfmChips params={params} {...mode} />;
    case "selcall":
      return <SelcallChips params={params} {...mode} />;
    case "am":
      return <AmChips params={params} {...mode} />;
    case "ssb":
      return <SsbChips params={params} {...mode} />;
    case "wfm":
      return <WfmChips params={params} {...mode} />;
    case "pocsag":
      return <PocsagChips params={params} {...mode} />;
    case "flex":
    case "ermes":
      return <PagerChips params={params} {...mode} />;
    case "adsb":
      return <AdsbChips params={params} {...mode} />;
    case "ais":
      return <AisChips params={params} {...mode} />;
    case "inmarsat_aero":
      return <AeroChips params={params} {...mode} />;
    case "iridium":
      return <IridiumChips params={params} {...mode} />;
    case "aprs":
      return <AprsChips params={params} {...mode} />;
    case "rtty":
      return <RttyChips params={params} {...mode} />;
    case "morse":
      return <MorseChips params={params} {...mode} />;
    case "cw_skimmer":
      return <SkimmerChips params={params} {...mode} />;
    case "ft8":
    case "ft4":
    case "wspr":
      return <WsjtChips params={params} {...mode} />;
    case "psk":
      return <PskChips params={params} {...mode} />;
    case "navtex":
      return <NavtexChips params={params} {...mode} />;
    case "radio_clock":
      return <RadioClockChips params={params} {...mode} />;
    case "gnss":
      return <GnssChips params={params} {...mode} />;
    case "vor":
      return <VorChips params={params} {...mode} />;
    case "ils":
      return <IlsChips params={params} {...mode} />;
    case "acars":
      return <AcarsChips params={params} {...mode} />;
    case "atv":
      return <AtvChips params={params} {...mode} />;
    case "sstv":
      return <SstvChips params={params} {...mode} />;
    case "dab":
      return <DabChips params={params} broadcast={broadcast} {...mode} />;
    case "datv":
      return <DatvChips params={params} broadcast={broadcast} {...mode} />;
    case "dvbt":
      return <DvbtChips params={params} broadcast={broadcast} {...mode} />;
    case "drm":
      return <DrmChips params={params} broadcast={broadcast} {...mode} />;
    case "dmr":
      return <DmrChips params={params} {...mode} />;
    case "nxdn":
      return <NxdnChips params={params} {...mode} />;
    case "freedv":
      return <FreedvChips params={params} {...mode} />;
    case "ident":
      return <IdentChips params={params} {...mode} />;
    case "dect":
      return <DectChips params={params} {...mode} />;
    case "apt":
      return <AptChips params={params} {...mode} />;
    case "lrpt":
      return <LrptChips params={params} {...mode} />;
    case "wefax":
      return <WefaxChips params={params} {...mode} />;
    case "radiosonde":
      return <RadiosondeChips params={params} {...mode} />;
    case "dstar":
    case "ysf":
    case "p25":
    case "dpmr":
    case "m17":
    case "dsc":
    case "inmarsat_stdc":
    case "vdl2":
    case "hfdl":
      return null;
    default:
      return unhandledMode(params);
  }
}

function unhandledMode(_params: never): null {
  return null;
}

function NfmChips({ params, limits, onParams }: Mode<"nfm">) {
  const settings = params.settings;
  const mode = settings.tone_mode ?? "off";
  const scrambler = settings.scrambler_mode ?? "off";
  const set = (next: typeof settings) => onParams({ type: "nfm", settings: next });
  return (
    <>
      <BandwidthChip
        valueHz={settings.bandwidth_hz ?? 12_500}
        optionsHz={[12_500, 25_000]}
        onCommit={(bandwidth_hz) => set({ ...settings, bandwidth_hz })}
      />
      <ChoiceChip
        label="Tone"
        title="Tone squelch"
        value={mode}
        options={NFM_TONE_MODES}
        quiet={mode === "off"}
        onChange={(tone_mode) =>
          set({
            ...settings,
            tone_mode,
            ctcss_hz: settings.ctcss_hz ?? CTCSS_DEFAULT_HZ,
            dcs_code: settings.dcs_code ?? DCS_DEFAULT_CODE,
          })
        }
      />
      {mode === "ctcss" && (
        <ChoiceChip
          label="CTCSS"
          title="CTCSS tone"
          value={settings.ctcss_hz ?? CTCSS_DEFAULT_HZ}
          options={CTCSS_OPTIONS}
          onChange={(ctcss_hz) => set({ ...settings, ctcss_hz })}
        />
      )}
      {mode === "dcs" && (
        <ChoiceChip
          label="DCS"
          title="DCS code"
          value={settings.dcs_code ?? DCS_DEFAULT_CODE}
          options={DCS_OPTIONS}
          onChange={(dcs_code) => set({ ...settings, dcs_code })}
        />
      )}
      <ChoiceChip
        label="Scrambler"
        title="Voice scrambler"
        value={scrambler}
        options={NFM_SCRAMBLER_MODES}
        quiet={scrambler === "off"}
        onChange={(scrambler_mode) =>
          set({
            ...settings,
            scrambler_mode,
            inversion_hz: settings.inversion_hz ?? INVERSION_DEFAULT_HZ,
          })
        }
      />
      {scrambler === "inversion" && (
        <HzChip
          label="Carrier"
          title="Inversion carrier"
          value={settings.inversion_hz ?? INVERSION_DEFAULT_HZ}
          limit={limitOf(limits, "inversion_hz")}
          onCommit={(inversion_hz) => set({ ...settings, inversion_hz })}
        />
      )}
      <ToggleChip
        label="Compander"
        title="Expand audio that was sent with 2:1 compression"
        on={settings.compander ?? false}
        onChange={(compander) => set({ ...settings, compander })}
      />
    </>
  );
}

function SelcallChips({ params, onParams }: Mode<"selcall">) {
  return (
    <ChoiceChip
      label="Plan"
      title="Selective calling tone plan"
      value={params.settings.system ?? "ccir1"}
      options={SELCALL_SYSTEMS}
      onChange={(system) => onParams({ type: "selcall", settings: { ...params.settings, system } })}
    />
  );
}

function AmChips({ params, onParams }: Mode<"am">) {
  return (
    <BandwidthChip
      valueHz={params.settings.bandwidth_hz ?? 10_000}
      optionsHz={[5_000, 8_000, 10_000]}
      onCommit={(bandwidth_hz) =>
        onParams({ type: "am", settings: { ...params.settings, bandwidth_hz } })
      }
    />
  );
}

function SsbChips({ params, limits, onParams }: Mode<"ssb">) {
  const settings = params.settings;
  return (
    <>
      <ChoiceChip
        label="Side"
        title="Sideband"
        value={settings.sideband ?? "usb"}
        options={SIDEBANDS}
        onChange={(sideband) => onParams({ type: "ssb", settings: { ...settings, sideband } })}
      />
      <HzChip
        label="BW"
        title="SSB bandwidth"
        value={settings.bandwidth_hz ?? 2_700}
        limit={limitOf(limits, "bandwidth_hz")}
        onCommit={(bandwidth_hz) =>
          onParams({ type: "ssb", settings: { ...settings, bandwidth_hz } })
        }
      />
    </>
  );
}

function WfmChips({ params, onParams }: Mode<"wfm">) {
  const settings = params.settings;
  return (
    <>
      <ChoiceChip
        label="De-emph"
        title="De-emphasis"
        value={settings.deemphasis_us ?? 50}
        options={DEEMPHASIS_US}
        onChange={(deemphasis_us) =>
          onParams({ type: "wfm", settings: { ...settings, deemphasis_us } })
        }
      />
      <ToggleChip
        label="Stereo"
        title="Decode the stereo pilot; mono is quieter on weak signals"
        on={settings.stereo ?? true}
        onChange={(stereo) => onParams({ type: "wfm", settings: { ...settings, stereo } })}
      />
    </>
  );
}

function PocsagChips({ params, onParams }: Mode<"pocsag">) {
  const settings = params.settings;
  const baud = settings.baud ?? "auto";
  return (
    <>
      <ChoiceChip
        label="Baud"
        title="POCSAG baud"
        value={baud}
        options={POCSAG_BAUDS}
        quiet={baud === "auto"}
        onChange={(next) => onParams({ type: "pocsag", settings: { ...settings, baud: next } })}
      />
      <BandwidthChip
        valueHz={settings.bandwidth_hz ?? 12_500}
        optionsHz={[12_500, 25_000]}
        onCommit={(bandwidth_hz) =>
          onParams({ type: "pocsag", settings: { ...settings, bandwidth_hz } })
        }
      />
      <InvertChip
        on={settings.invert ?? false}
        onChange={(invert) => onParams({ type: "pocsag", settings: { ...settings, invert } })}
      />
    </>
  );
}

function PagerChips({ params, onParams }: Mode<"flex" | "ermes">) {
  const { type, settings } = params;
  return (
    <>
      <BandwidthChip
        valueHz={settings.bandwidth_hz ?? 12_500}
        optionsHz={[12_500, 25_000]}
        onCommit={(bandwidth_hz) => onParams({ type, settings: { ...settings, bandwidth_hz } })}
      />
      <InvertChip
        on={settings.invert ?? false}
        onChange={(invert) => onParams({ type, settings: { ...settings, invert } })}
      />
    </>
  );
}

function AdsbChips({ params, onParams }: Mode<"adsb">) {
  return (
    <ToggleChip
      label="CRC fix"
      title="Repair single-bit errors the checksum can pin down"
      on={params.settings.crc_fix ?? true}
      onChange={(crc_fix) => onParams({ type: "adsb", settings: { ...params.settings, crc_fix } })}
    />
  );
}

function AisChips({ params, onParams }: Mode<"ais">) {
  return (
    <ChoiceChip
      label="Channel"
      title="AIS channel"
      value={params.settings.ais_channel ?? "a"}
      options={AIS_CHANNELS}
      onChange={(ais_channel) =>
        onParams({ type: "ais", settings: { ...params.settings, ais_channel } })
      }
    />
  );
}

function AeroChips({ params, onParams }: Mode<"inmarsat_aero">) {
  return (
    <ChoiceChip
      label="Channel"
      title="Aero channel"
      value={params.settings.channel ?? "p"}
      options={AERO_CHANNELS}
      onChange={(channel) =>
        onParams({ type: "inmarsat_aero", settings: { ...params.settings, channel } })
      }
    />
  );
}

function IridiumChips({ params, onParams }: Mode<"iridium">) {
  return (
    <ChoiceChip
      label="Span"
      title="Iridium span"
      value={params.settings.span ?? "channel"}
      options={IRIDIUM_SPANS}
      onChange={(span) => onParams({ type: "iridium", settings: { ...params.settings, span } })}
    />
  );
}

function AprsChips({ params, onParams }: Mode<"aprs">) {
  const settings = params.settings;
  return (
    <>
      <ChoiceChip
        label="Mode"
        title="APRS mode"
        value={settings.mode ?? "afsk1200"}
        options={APRS_MODES}
        onChange={(mode) => onParams({ type: "aprs", settings: { ...settings, mode } })}
      />
      <BandwidthChip
        valueHz={settings.bandwidth_hz ?? 12_500}
        optionsHz={[12_500, 25_000]}
        onCommit={(bandwidth_hz) =>
          onParams({ type: "aprs", settings: { ...settings, bandwidth_hz } })
        }
      />
    </>
  );
}

function RttyChips({ params, limits, onParams }: Mode<"rtty">) {
  const settings = params.settings;
  const set = (next: typeof settings) => onParams({ type: "rtty", settings: next });
  return (
    <>
      <PresetChip
        label="Baud"
        title="RTTY baud"
        value={settings.baud ?? 45.45}
        presets={RTTY_BAUDS}
        limit={limitOf(limits, "baud")}
        onCommit={(baud) => set({ ...settings, baud })}
      />
      <PresetChip
        label="Shift"
        title="RTTY shift"
        unit="Hz"
        value={settings.shift_hz ?? 170}
        presets={RTTY_SHIFTS_HZ}
        limit={limitOf(limits, "shift_hz")}
        onCommit={(shift_hz) => set({ ...settings, shift_hz })}
      />
      <ChoiceChip
        label="Stop"
        title="RTTY stop bits"
        value={settings.stop_bits ?? "one_and_half"}
        options={RTTY_STOP_BITS}
        onChange={(stop_bits) => set({ ...settings, stop_bits })}
      />
      <InvertChip
        on={settings.invert ?? false}
        onChange={(invert) => set({ ...settings, invert })}
      />
      <ToggleChip
        label="Unshift"
        title="Drop back to letters after a space, as most stations expect"
        on={settings.unshift_on_space ?? true}
        onChange={(unshift_on_space) => set({ ...settings, unshift_on_space })}
      />
    </>
  );
}

function MorseChips({ params, limits, onParams }: Mode<"morse">) {
  const settings = params.settings;
  return (
    <>
      <HzChip
        label="BW"
        title="CW filter bandwidth"
        value={settings.bandwidth_hz ?? 400}
        limit={limitOf(limits, "bandwidth_hz")}
        onCommit={(bandwidth_hz) =>
          onParams({ type: "morse", settings: { ...settings, bandwidth_hz } })
        }
      />
      <OptionalNumberChip
        label="Speed"
        title="Morse speed, empty to auto-track"
        placeholder="auto"
        value={settings.wpm ?? null}
        {...limitOf(limits, "wpm")}
        unit="WPM"
        onCommit={(wpm) => onParams({ type: "morse", settings: { ...settings, wpm } })}
      />
    </>
  );
}

function SkimmerChips({ params, limits, onParams }: Mode<"cw_skimmer">) {
  const settings = params.settings;
  const set = (next: typeof settings) => onParams({ type: "cw_skimmer", settings: next });
  return (
    <>
      <HzChip
        label="Passband"
        title="CW skimmer passband"
        value={settings.bandwidth_hz ?? 24_000}
        limit={limitOf(limits, "bandwidth_hz")}
        onCommit={(bandwidth_hz) => set({ ...settings, bandwidth_hz })}
      />
      <NumberChip
        label="Acquire"
        title="Carrier threshold above the noise floor"
        value={settings.threshold_db ?? 10}
        {...limitOf(limits, "threshold_db")}
        unit="dB SNR"
        onCommit={(threshold_db) => set({ ...settings, threshold_db })}
      />
      <NumberChip
        label="Signals"
        title="Maximum simultaneous CW signals"
        value={settings.max_signals ?? 32}
        {...limitOf(limits, "max_signals")}
        onCommit={(max_signals) => set({ ...settings, max_signals })}
      />
      <OptionalNumberChip
        label="Speed"
        title="Morse speed, empty to track each signal"
        placeholder="auto"
        value={settings.wpm ?? null}
        {...limitOf(limits, "wpm")}
        unit="WPM"
        onCommit={(wpm) => set({ ...settings, wpm })}
      />
    </>
  );
}

function WsjtChips({ params, limits, onParams }: Mode<"ft8" | "ft4" | "wspr">) {
  const { type, settings } = params;
  const wspr = type === "wspr";
  return (
    <>
      <HzChip
        label="From"
        title="Lowest USB audio frequency searched"
        value={settings.audio_low_hz ?? (wspr ? 1_400 : 200)}
        limit={limitOf(limits, "audio_low_hz")}
        onCommit={(audio_low_hz) => onParams({ type, settings: { ...settings, audio_low_hz } })}
      />
      <HzChip
        label="To"
        title="Highest USB audio frequency searched"
        value={settings.audio_high_hz ?? (wspr ? 1_600 : 3_000)}
        limit={limitOf(limits, "audio_high_hz")}
        onCommit={(audio_high_hz) => onParams({ type, settings: { ...settings, audio_high_hz } })}
      />
      <NumberChip
        label="Candidates"
        title="Maximum synchronized signals tried per decode pass"
        value={settings.max_candidates ?? 200}
        {...limitOf(limits, "max_candidates")}
        onCommit={(max_candidates) => onParams({ type, settings: { ...settings, max_candidates } })}
      />
    </>
  );
}

function PskChips({ params, onParams }: Mode<"psk">) {
  const settings = params.settings;
  return (
    <>
      <ChoiceChip
        label="Mode"
        title="PSK symbol rate"
        value={settings.baud ?? "psk31"}
        options={PSK_BAUDS}
        onChange={(baud) => onParams({ type: "psk", settings: { ...settings, baud } })}
      />
      <InvertChip
        on={settings.invert ?? false}
        onChange={(invert) => onParams({ type: "psk", settings: { ...settings, invert } })}
      />
    </>
  );
}

function NavtexChips({ params, onParams }: Mode<"navtex">) {
  return (
    <InvertChip
      on={params.settings.invert ?? false}
      onChange={(invert) => onParams({ type: "navtex", settings: { ...params.settings, invert } })}
    />
  );
}

function RadioClockChips({ params, onParams }: Mode<"radio_clock">) {
  const settings = params.settings;
  return (
    <>
      <ChoiceChip
        label="Service"
        title="Radio clock service"
        value={settings.standard ?? "dcf77"}
        options={RADIO_CLOCK_STANDARDS}
        onChange={(standard) =>
          onParams({ type: "radio_clock", settings: { ...settings, standard } })
        }
      />
      <InvertChip
        on={settings.invert ?? false}
        onChange={(invert) => onParams({ type: "radio_clock", settings: { ...settings, invert } })}
      />
    </>
  );
}

function GnssChips({ params, limits, onParams }: Mode<"gnss">) {
  const settings = params.settings;
  return (
    <>
      <NumberChip
        label="PRN"
        title="GPS L1 C/A satellite PRN"
        value={settings.prn ?? 1}
        {...limitOf(limits, "prn")}
        onCommit={(prn) => onParams({ type: "gnss", settings: { ...settings, prn } })}
      />
      <HzChip
        label="Doppler"
        title="Symmetric Doppler search span"
        value={settings.doppler_hz ?? 10_000}
        limit={limitOf(limits, "doppler_hz")}
        onCommit={(doppler_hz) => onParams({ type: "gnss", settings: { ...settings, doppler_hz } })}
      />
      <NumberChip
        label="Acquire"
        title="Correlation peak-to-floor acquisition threshold"
        value={settings.threshold ?? 2.5}
        {...limitOf(limits, "threshold")}
        unit="× floor"
        onCommit={(threshold) => onParams({ type: "gnss", settings: { ...settings, threshold } })}
      />
    </>
  );
}

function VorChips({ params, limits, onParams }: Mode<"vor">) {
  const settings = params.settings;
  const set = (next: typeof settings) => onParams({ type: "vor", settings: next });
  const station = settings.station ?? "";
  return (
    <>
      <SettingChip
        label="Station"
        value={station === "" ? "none" : station}
        quiet={station === ""}
        title="VOR station identifier"
      >
        {() => (
          <ChipField label="VOR station identifier">
            <TextAutocomplete
              className="min-w-0 flex-1"
              label="VOR station identifier"
              value={station}
              suggestions={[]}
              placeholder="Optional identifier"
              onCommit={(next) => {
                set({ ...settings, station: next === "" ? undefined : next });
                return true;
              }}
            />
          </ChipField>
        )}
      </SettingChip>
      <OptionalNumberChip
        label="Lat"
        title="VOR station latitude"
        placeholder="Unknown"
        value={settings.station_lat ?? null}
        {...limitOf(limits, "station_lat")}
        unit="°"
        onCommit={(station_lat) => set({ ...settings, station_lat })}
      />
      <OptionalNumberChip
        label="Lon"
        title="VOR station longitude"
        placeholder="Unknown"
        value={settings.station_lon ?? null}
        {...limitOf(limits, "station_lon")}
        unit="°"
        onCommit={(station_lon) => set({ ...settings, station_lon })}
      />
      <NumberChip
        label="Declination"
        title="East-positive magnetic declination at the VOR"
        value={settings.magnetic_declination_deg ?? 0}
        {...limitOf(limits, "magnetic_declination_deg")}
        unit="°"
        onCommit={(magnetic_declination_deg) => set({ ...settings, magnetic_declination_deg })}
      />
      <NumberChip
        label="Every"
        title="VOR report interval"
        value={settings.report_ms ?? 500}
        {...limitOf(limits, "report_ms")}
        unit="ms"
        onCommit={(report_ms) => set({ ...settings, report_ms })}
      />
    </>
  );
}

function IlsChips({ params, limits, onParams }: Mode<"ils">) {
  const settings = params.settings;
  return (
    <>
      <ChoiceChip
        label="Component"
        title="ILS component"
        value={settings.component ?? "localizer"}
        options={ILS_COMPONENTS}
        onChange={(component) => onParams({ type: "ils", settings: { ...settings, component } })}
      />
      <NumberChip
        label="Every"
        title="ILS report interval"
        value={settings.report_ms ?? 500}
        {...limitOf(limits, "report_ms")}
        unit="ms"
        onCommit={(report_ms) => onParams({ type: "ils", settings: { ...settings, report_ms } })}
      />
    </>
  );
}

function AcarsChips({ params, onParams }: Mode<"acars">) {
  return (
    <BandwidthChip
      valueHz={params.settings.bandwidth_hz ?? 12_500}
      optionsHz={[8_000, 12_500, 25_000]}
      onCommit={(bandwidth_hz) =>
        onParams({ type: "acars", settings: { ...params.settings, bandwidth_hz } })
      }
    />
  );
}

function AtvChips({ params, limits, onParams }: Mode<"atv">) {
  const settings = params.settings;
  const set = (next: typeof settings) => onParams({ type: "atv", settings: next });
  return (
    <>
      <ChoiceChip
        label="Mod"
        title="Modulation"
        value={settings.modulation ?? "am"}
        options={ATV_MODULATIONS}
        onChange={(modulation) => set({ ...settings, modulation })}
      />
      <ChoiceChip
        label="Lines"
        title="Scanning standard"
        value={settings.standard ?? "ccir625"}
        options={ATV_STANDARDS}
        onChange={(standard) => set({ ...settings, standard })}
      />
      <BandwidthChip
        valueHz={settings.bandwidth_hz ?? 1_500_000}
        optionsHz={[500_000, 1_000_000, 1_500_000, 1_600_000]}
        onCommit={(bandwidth_hz) => set({ ...settings, bandwidth_hz })}
      />
      <ChoiceChip
        label="Colour"
        title="Composite colour system"
        value={settings.color ?? "monochrome"}
        options={ATV_COLORS}
        onChange={(color) => set({ ...settings, color })}
      />
      <OptionalNumberChip
        label="Sound"
        title="FM sound subcarrier, empty for none"
        placeholder="off"
        value={
          settings.sound_subcarrier_hz == null ? null : settings.sound_subcarrier_hz / 1_000_000
        }
        {...scaledLimit(limitOf(limits, "sound_subcarrier_hz"), 1e-6)}
        unit="MHz"
        onCommit={(mhz) =>
          set({ ...settings, sound_subcarrier_hz: mhz === null ? null : mhz * 1_000_000 })
        }
      />
      <ToggleChip
        label="Interlace"
        title="Weave both fields into one frame"
        on={settings.interlace ?? true}
        onChange={(interlace) => set({ ...settings, interlace })}
      />
      <InvertChip
        on={settings.invert ?? false}
        onChange={(invert) => set({ ...settings, invert })}
      />
    </>
  );
}

function SstvChips({ params, onParams }: Mode<"sstv">) {
  const settings = params.settings;
  const mode = settings.mode ?? SSTV_AUTO;
  return (
    <>
      <ChoiceChip
        label="Mode"
        title="Scanning mode"
        value={mode}
        options={SSTV_MODES}
        quiet={mode === SSTV_AUTO}
        onChange={(next) =>
          onParams({
            type: "sstv",
            settings: { ...settings, mode: next === SSTV_AUTO ? null : next },
          })
        }
      />
      <ToggleChip
        label="Slant"
        title="Straighten pictures from a sender whose clock runs off"
        on={settings.slant_correction ?? true}
        onChange={(slant_correction) =>
          onParams({ type: "sstv", settings: { ...settings, slant_correction } })
        }
      />
      <ToggleChip
        label="Partial"
        title="Keep unfinished pictures"
        on={settings.keep_partial ?? true}
        onChange={(keep_partial) =>
          onParams({ type: "sstv", settings: { ...settings, keep_partial } })
        }
      />
    </>
  );
}

function DabChips({ params, broadcast, onParams }: Mode<"dab"> & { broadcast?: BroadcastStatus }) {
  const settings = params.settings;
  const mode = settings.mode ?? "auto";
  const transmissionMode = settings.transmission_mode ?? "i";
  const detected = broadcast?.transmission_mode;
  const transmissionModes =
    detected == null
      ? DAB_TRANSMISSION_MODES
      : DAB_TRANSMISSION_MODES.map((option) =>
          option.value === "auto" ? { ...option, label: `Auto ${detected.toUpperCase()}` } : option,
        );
  return (
    <>
      <ChoiceChip
        label="Type"
        title="DAB generation"
        value={mode}
        options={DAB_MODES}
        quiet={mode === "auto"}
        onChange={(next) => onParams({ type: "dab", settings: { ...settings, mode: next } })}
      />
      <ChoiceChip
        label="Mode"
        title="DAB transmission mode"
        value={transmissionMode}
        options={transmissionModes}
        quiet={transmissionMode === "auto"}
        onChange={(transmission_mode) =>
          onParams({ type: "dab", settings: { ...settings, transmission_mode } })
        }
      />
      <ServiceChip
        status={broadcast}
        value={settings.service_id ?? null}
        max={0xffffffff}
        onChange={(service_id) => onParams({ type: "dab", settings: { ...settings, service_id } })}
      />
    </>
  );
}

function DatvChips({
  params,
  broadcast,
  limits,
  onParams,
}: Mode<"datv"> & { broadcast?: BroadcastStatus }) {
  const settings = params.settings;
  const set = (next: typeof settings) => onParams({ type: "datv", settings: next });
  const codeRate = settings.code_rate ?? "auto";
  return (
    <>
      <ChoiceChip
        label="Standard"
        title="DATV standard"
        value={settings.standard ?? "dvb_s"}
        options={DATV_STANDARDS}
        onChange={(standard) => set({ ...settings, standard })}
      />
      <NumberChip
        label="Rate"
        title="DATV symbol rate"
        value={settings.symbol_rate ?? 333_000}
        {...limitOf(limits, "symbol_rate")}
        unit="Bd"
        onCommit={(symbol_rate) => set({ ...settings, symbol_rate })}
      />
      <ServiceChip
        status={broadcast}
        value={settings.program ?? null}
        max={65535}
        onChange={(program) => set({ ...settings, program })}
      />
      {settings.standard === "dvb_s2" ? (
        <>
          <ChoiceChip
            label="Roll-off"
            title="Match the transmitter's filter shape; DVB-S2 signals it in the base band header"
            value={settings.roll_off ?? "pct35"}
            options={DATV_ROLL_OFFS}
            onChange={(roll_off) => set({ ...settings, roll_off })}
          />
          <ToggleChip
            label="Superframes"
            title="Receive DVB-S2X Annex E superframes, formats 0 to 7"
            on={settings.superframes ?? false}
            onChange={(superframes) => set({ ...settings, superframes })}
          />
          {settings.superframes ? (
            <>
              <NumberChip
                label="Ref code"
                title="Superframe reference scrambling code n_Ref, 0 by default"
                value={settings.superframe_reference ?? 0}
                {...limitOf(limits, "superframe_reference")}
                onCommit={(superframe_reference) => set({ ...settings, superframe_reference })}
              />
              <NumberChip
                label="Data code"
                title="Superframe payload scrambling code n_Pay, 0 by default"
                value={settings.superframe_payload ?? 0}
                {...limitOf(limits, "superframe_payload")}
                onCommit={(superframe_payload) => set({ ...settings, superframe_payload })}
              />
              <ToggleChip
                label="Code search"
                title="Find unknown scrambling codes from the signal; needs a clean carrier"
                on={settings.superframe_search ?? false}
                onChange={(superframe_search) => set({ ...settings, superframe_search })}
              />
            </>
          ) : null}
          <OptionalNumberChip
            label="Stream"
            title="Choose an input stream identifier on a multistream carrier"
            placeholder="Auto"
            value={settings.input_stream ?? null}
            min={0}
            max={255}
            step={1}
            onCommit={(input_stream) => set({ ...settings, input_stream })}
          />
        </>
      ) : (
        <ChoiceChip
          label="FEC"
          title="DVB-S code rate"
          value={codeRate}
          options={DATV_CODE_RATES}
          quiet={codeRate === "auto"}
          onChange={(code_rate) => set({ ...settings, code_rate })}
        />
      )}
    </>
  );
}

function DvbtChips({
  params,
  broadcast,
  onParams,
}: Mode<"dvbt"> & { broadcast?: BroadcastStatus }) {
  const settings = params.settings;
  const set = (next: typeof settings) => onParams({ type: "dvbt", settings: next });
  return (
    <>
      <ChoiceChip
        label="Standard"
        title="Terrestrial standard"
        value={settings.standard ?? "dvb_t"}
        options={DVBT_STANDARDS}
        onChange={(standard) => set({ ...settings, standard })}
      />
      <ChoiceChip
        label="BW"
        title="DVB-T bandwidth"
        value={settings.bandwidth ?? "mhz8"}
        options={DVBT_BANDWIDTHS}
        onChange={(bandwidth) => set({ ...settings, bandwidth })}
      />
      {settings.standard === "dvb_t2" ? (
        <OptionalNumberChip
          label="PLP"
          title="PLP ID, empty for automatic selection"
          placeholder="auto"
          value={settings.plp ?? null}
          min={0}
          max={255}
          step={1}
          onCommit={(plp) => set({ ...settings, plp })}
        />
      ) : (
        <ToggleChip
          label="Low priority"
          title="Decode the low priority transport stream of a hierarchical DVB-T multiplex"
          on={settings.low_priority ?? false}
          onChange={(low_priority) => set({ ...settings, low_priority })}
        />
      )}
      <ServiceChip
        status={broadcast}
        value={settings.program ?? null}
        max={65535}
        onChange={(program) => set({ ...settings, program })}
      />
    </>
  );
}

function DrmChips({ params, broadcast, onParams }: Mode<"drm"> & { broadcast?: BroadcastStatus }) {
  const settings = params.settings;
  const mode = settings.mode ?? "auto";
  return (
    <>
      <ChoiceChip
        label="Mode"
        title="DRM mode"
        value={mode}
        options={DRM_MODES}
        quiet={mode === "auto"}
        onChange={(next) =>
          onParams({
            type: "drm",
            settings: {
              ...settings,
              mode: next,
              bandwidth_hz:
                next === "drm30" ? (mode === "drm30" ? settings.bandwidth_hz : 10_000) : 100_000,
            },
          })
        }
      />
      <BandwidthChip
        valueHz={settings.bandwidth_hz ?? 100_000}
        optionsHz={mode === "drm30" ? [4_500, 5_000, 9_000, 10_000, 18_000, 20_000] : [100_000]}
        onCommit={(bandwidth_hz) =>
          onParams({ type: "drm", settings: { ...settings, bandwidth_hz } })
        }
      />
      <ServiceChip
        status={broadcast}
        value={settings.service ?? null}
        max={3}
        onChange={(service) => onParams({ type: "drm", settings: { ...settings, service } })}
      />
    </>
  );
}

function DmrChips({ params, onParams }: Mode<"dmr">) {
  const settings = params.settings;
  return (
    <>
      <ChoiceChip
        label="Slot"
        title="Slot"
        value={settings.slots ?? "both"}
        options={DMR_SLOTS}
        onChange={(slots) => onParams({ type: "dmr", settings: { ...settings, slots } })}
      />
      <ToggleChip
        label="Skip CRC"
        title="Ignore data CRC"
        on={settings.ignore_crc ?? false}
        onChange={(ignore_crc) => onParams({ type: "dmr", settings: { ...settings, ignore_crc } })}
      />
    </>
  );
}

function NxdnChips({ params, onParams }: Mode<"nxdn">) {
  return (
    <ChoiceChip
      label="Width"
      title="Width"
      value={params.settings.bandwidth ?? "narrow"}
      options={NXDN_WIDTHS}
      onChange={(bandwidth) =>
        onParams({ type: "nxdn", settings: { ...params.settings, bandwidth } })
      }
    />
  );
}

function FreedvChips({ params, onParams }: Mode<"freedv">) {
  return (
    <ChoiceChip
      label="Side"
      title="FreeDV sideband"
      value={params.settings.sideband ?? "usb"}
      options={SIDEBANDS}
      onChange={(sideband) =>
        onParams({ type: "freedv", settings: { ...params.settings, sideband } })
      }
    />
  );
}

function IdentChips({ params, limits, onParams }: Mode<"ident">) {
  const settings = params.settings;
  const set = (next: typeof settings) => onParams({ type: "ident", settings: next });
  return (
    <>
      <BandwidthChip
        label="Width"
        title="Search bandwidth"
        valueHz={settings.bandwidth_hz ?? 192_000}
        optionsHz={[12_500, 50_000, 100_000, 192_000]}
        onCommit={(bandwidth_hz) => set({ ...settings, bandwidth_hz })}
      />
      <NumberChip
        label="Every"
        title="Milliseconds of signal each report is measured from"
        value={settings.interval_ms ?? 1_000}
        {...limitOf(limits, "interval_ms")}
        unit="ms"
        onCommit={(interval_ms) => set({ ...settings, interval_ms })}
      />
      <NumberChip
        label="Detect"
        title="Decibels above the noise floor a signal must reach"
        value={settings.threshold_db ?? 8}
        {...limitOf(limits, "threshold_db")}
        unit="dB"
        onCommit={(threshold_db) => set({ ...settings, threshold_db })}
      />
    </>
  );
}

function DectChips({ params, onParams }: Mode<"dect">) {
  const settings = params.settings;
  return (
    <>
      <ChoiceChip
        label="Band"
        title="DECT band"
        value={settings.band ?? "eu"}
        options={DECT_BANDS}
        onChange={(band) => onParams({ type: "dect", settings: { ...settings, band } })}
      />
      <ChoiceChip
        label="Side"
        title="DECT side"
        value={settings.sides ?? "both"}
        options={DECT_SIDES}
        onChange={(sides) => onParams({ type: "dect", settings: { ...settings, sides } })}
      />
    </>
  );
}

function AptChips({ params, onParams }: Mode<"apt">) {
  const settings = params.settings;
  return (
    <ToggleChip
      label="Partial"
      title="Keep unfinished passes"
      on={settings.keep_partial ?? true}
      onChange={(keep_partial) =>
        onParams({ type: "apt", settings: { ...settings, keep_partial } })
      }
    />
  );
}

function LrptChips({ params, onParams }: Mode<"lrpt">) {
  const settings = params.settings;
  return (
    <ChoiceChip
      label="Mode"
      title="Meteor-M downlink mode"
      value={settings.mode ?? "oqpsk72"}
      options={LRPT_MODES}
      onChange={(mode) => onParams({ type: "lrpt", settings: { ...settings, mode } })}
    />
  );
}

function WefaxChips({ params, onParams }: Mode<"wefax">) {
  const settings = params.settings;
  const set = (next: typeof settings) => onParams({ type: "wefax", settings: next });
  return (
    <>
      <ChoiceChip
        label="IOC"
        title="Index of cooperation"
        value={settings.ioc ?? "ioc576"}
        options={WEFAX_IOCS}
        onChange={(ioc) => set({ ...settings, ioc })}
      />
      <ChoiceChip
        label="LPM"
        title="Lines per minute"
        value={settings.lpm ?? "lpm120"}
        options={WEFAX_LPMS}
        onChange={(lpm) => set({ ...settings, lpm })}
      />
      <ToggleChip
        label="Partial"
        title="Keep unfinished charts"
        on={settings.keep_partial ?? true}
        onChange={(keep_partial) => set({ ...settings, keep_partial })}
      />
    </>
  );
}

function RadiosondeChips({ params, onParams }: Mode<"radiosonde">) {
  const settings = params.settings;
  const sonde = settings.sonde ?? SONDE_AUTO;
  return (
    <ChoiceChip
      label="Sonde"
      title="Sonde type"
      value={sonde}
      options={SONDE_TYPES}
      quiet={sonde === SONDE_AUTO}
      onChange={(next) =>
        onParams({
          type: "radiosonde",
          settings: { ...settings, sonde: next === SONDE_AUTO ? null : next },
        })
      }
    />
  );
}

export function bandwidthOptions(valueHz: number, optionsHz: readonly number[]): Options<number> {
  return withCurrent(
    valueHz,
    optionsHz.map((hz) => ({ value: hz, label: formatHz(hz) })),
    formatHz,
  );
}

function BandwidthChip({
  label = "BW",
  title = "Channel bandwidth",
  valueHz,
  optionsHz,
  onCommit,
}: {
  label?: string;
  title?: string;
  valueHz: number;
  optionsHz: readonly number[];
  onCommit: (hz: number) => void;
}) {
  return (
    <ChoiceChip
      label={label}
      title={title}
      value={valueHz}
      options={bandwidthOptions(valueHz, optionsHz)}
      onChange={onCommit}
    />
  );
}

function HzChip({
  label,
  title,
  value,
  limit,
  onCommit,
}: {
  label: string;
  title: string;
  value: number;
  limit: NumberLimit;
  onCommit: (hz: number) => void;
}) {
  return (
    <NumberChip
      label={label}
      title={title}
      value={value}
      shown={formatHz(value)}
      unit="Hz"
      {...limit}
      onCommit={onCommit}
    />
  );
}

function InvertChip({ on, onChange }: { on: boolean; onChange: (on: boolean) => void }) {
  return <ToggleChip label="Invert" title={INVERT_TITLE} on={on} onChange={onChange} />;
}

function PresetChip({
  label,
  title,
  value,
  presets,
  limit,
  unit,
  onCommit,
}: {
  label: string;
  title: string;
  value: number;
  presets: Options<number>;
  limit: NumberLimit;
  unit?: string;
  onCommit: (value: number) => void;
}) {
  return (
    <SettingChip label={label} value={String(value)} unit={unit} title={title}>
      {() => (
        <ChipField label={title}>
          <Segmented
            label={`${title} presets`}
            value={value}
            options={presets}
            onChange={onCommit}
          />
          <NumberField
            className="min-w-0 flex-1"
            label={title}
            value={value}
            unit={unit}
            {...limit}
            onCommit={onCommit}
          />
        </ChipField>
      )}
    </SettingChip>
  );
}

export function serviceOptions(
  services: readonly { id: number; label: string }[],
  value: number | null,
): Options<string> {
  const options = [
    { value: "", label: "Auto" },
    ...services.map((service) => ({
      value: String(service.id),
      label: service.label || String(service.id),
    })),
  ];
  if (value !== null && !services.some((service) => service.id === value)) {
    options.push({ value: String(value), label: String(value) });
  }
  return options;
}

function ServiceChip({
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
  if (services.length === 0) {
    return (
      <OptionalNumberChip
        label="Service"
        title={SERVICE_TITLE}
        placeholder="Auto"
        value={value}
        min={0}
        max={max}
        step={1}
        onCommit={onChange}
      />
    );
  }
  return (
    <ChoiceChip
      label="Service"
      title={SERVICE_TITLE}
      value={value === null ? "" : String(value)}
      options={serviceOptions(services, value)}
      quiet={value === null}
      onChange={(id) => onChange(id === "" ? null : Number(id))}
    />
  );
}
