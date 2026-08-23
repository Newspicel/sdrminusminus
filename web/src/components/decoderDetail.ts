import type {
  AprsPacket,
  DataLinkMessage,
  DecoderEvent,
  DecoderKind,
  DectFrame,
  DvFrame,
} from "../lib/types";
import { hex2, hex5 } from "./decoderLog";
import {
  candidateScore,
  dvChecksum,
  dvMode,
  dvNetwork,
  dvParties,
  dvTrunking,
  identMeasurements,
  modulationLabel,
} from "./decoderViews";
import { DECT_CIPHER_LABELS } from "./eventFacts";
import { SSTV_MODE_LABELS } from "./sstvModes";

export type DetailField = readonly [label: string, value: string];

export interface EventDetail {
  fields: DetailField[];
  body: string | null;
}

export function eventDetail(event: DecoderEvent): EventDetail {
  const build = DETAIL[event.kind] as (data: DecoderEvent["data"]) => EventDetail;
  return build(event.data);
}

const DETAIL: {
  [K in DecoderKind]: (data: Extract<DecoderEvent, { kind: K }>["data"]) => EventDetail;
} = {
  rds: (r) => ({
    fields: fields([
      ["PI", r.pi],
      ["Station", r.ps?.trim()],
      ["Programme type", r.pty_name ?? (r.pty == null ? undefined : `PTY ${r.pty}`)],
      ["Traffic programme", flag(r.tp)],
      ["Traffic announcement", flag(r.ta)],
      ["Content", r.music == null ? undefined : r.music ? "music" : "speech"],
      ["Alternative frequencies", (r.alt_freqs_hz ?? []).map(mhz).join(", ")],
      ["Groups", String(r.groups)],
      ["Block errors", String(r.block_errors)],
    ]),
    body: r.radiotext?.trim() || null,
  }),

  pocsag: (p) => ({
    fields: fields([
      ["RIC", String(p.address).padStart(7, "0")],
      ["Function", `${"ABCD"[p.function] ?? p.function} (${p.function})`],
      ["Baud", String(p.baud)],
      ["Payload", p.payload],
      ["Repaired", p.errors_corrected > 0 ? String(p.errors_corrected) : undefined],
    ]),
    body: p.text || null,
  }),

  flex: (p) => ({
    fields: fields([
      ["Address", String(p.address)],
      ["Payload", p.payload],
      ["Mode", `${p.baud}/${p.levels}`],
      ["Cycle", String(p.cycle)],
      ["Frame", String(p.frame)],
      ["Phase", p.phase],
      ["Repaired", p.errors_corrected > 0 ? String(p.errors_corrected) : undefined],
    ]),
    body: p.text || null,
  }),

  ermes: (p) => ({
    fields: fields([
      ["Local address", String(p.local_address)],
      ["Message number", String(p.message_number)],
      ["Payload", p.payload],
      ["Urgent", flag(p.urgent)],
      ["Alert", String(p.alert)],
      ["Repaired", p.errors_corrected > 0 ? String(p.errors_corrected) : undefined],
    ]),
    body: p.text || null,
  }),

  adsb: (a) => ({
    fields: fields([
      ["ICAO", a.icao.toUpperCase()],
      ["Callsign", a.callsign?.trim()],
      ["Downlink format", String(a.df)],
      ["Type code", a.type_code == null ? undefined : String(a.type_code)],
      ["Altitude", a.on_ground === true ? "on ground" : feet(a.altitude_ft)],
      ["Position", position(a.lat, a.lon)],
      ["Ground speed", knots(a.ground_speed_kt)],
      ["Track", degrees(a.track_deg)],
      ["Vertical rate", a.vertical_rate_fpm == null ? undefined : `${a.vertical_rate_fpm} ft/min`],
      ["Squawk", a.squawk],
      ["Raw", a.raw],
    ]),
    body: null,
  }),

  ais: (m) => ({
    fields: fields([
      ["MMSI", String(m.mmsi)],
      ["Message type", String(m.msg_type)],
      ["Channel", m.ais_channel],
      ["Name", m.name?.trim()],
      ["Call sign", m.call_sign?.trim()],
      ["Destination", m.destination?.trim()],
      ["Position", position(m.lat, m.lon)],
      ["Speed over ground", knots(m.sog_kt)],
      ["Course over ground", degrees(m.cog_deg)],
      ["Heading", degrees(m.heading_deg)],
      ["Navigational status", m.nav_status == null ? undefined : String(m.nav_status)],
    ]),
    body: m.nmea || null,
  }),

  aprs: (p) => ({
    fields: fields([
      ["Source", p.source],
      ["Destination", p.destination],
      ["Path", (p.path ?? []).join(" → ")],
      ["Symbol", p.symbol],
      ["Position", position(p.lat, p.lon)],
      ["Course", degrees(p.course_deg)],
      ["Speed", knots(p.speed_kt)],
      ["Altitude", feet(p.altitude_ft)],
      ["Mic-E message", p.mic_e_message],
      ["Comment", p.comment?.trim()],
    ]),
    body: monitorLine(p),
  }),

  rtty: (t) => ({ fields: [], body: t.text || null }),

  morse: (m) => ({ fields: fields([["Speed", `${m.wpm.toFixed(0)} WPM`]]), body: m.text || null }),

  cw_skimmer: (m) => ({
    fields: fields([
      ["Offset", `${signed(m.offset_hz, 0)} Hz`],
      ["Speed", `${m.wpm.toFixed(0)} WPM`],
      ["SNR", `${signed(m.snr_db, 1)} dB`],
    ]),
    body: m.text || null,
  }),

  ft8: (m) => ({
    fields: fields([
      ["SNR", `${signed(m.snr_db, 0)} dB`],
      ["Audio", `${m.audio_hz.toFixed(1)} Hz`],
      ["Time offset", `${signed(m.time_offset_s, 2)} s`],
      ["Hard errors", String(m.hard_errors)],
    ]),
    body: m.text || null,
  }),

  ft4: (m) => ({
    fields: fields([
      ["SNR", `${signed(m.snr_db, 0)} dB`],
      ["Audio", `${m.audio_hz.toFixed(1)} Hz`],
      ["Time offset", `${signed(m.time_offset_s, 2)} s`],
      ["Hard errors", String(m.hard_errors)],
    ]),
    body: m.text || null,
  }),

  psk: (t) => ({ fields: fields([["Mode", t.baud.toUpperCase()]]), body: t.text || null }),

  wspr: (s) => ({
    fields: fields([
      ["Callsign", s.callsign],
      ["Grid", s.grid],
      ["Power", `${s.power_dbm} dBm`],
      ["SNR", `${signed(s.snr_db, 0)} dB`],
      ["Audio", `${s.audio_hz.toFixed(1)} Hz`],
      ["Time offset", `${signed(s.time_offset_s, 2)} s`],
      ["Drift", `${signed(s.drift_hz, 1)} Hz`],
    ]),
    body: s.text || null,
  }),

  selcall: (s) => ({
    fields: fields([
      ["Tone plan", s.system === "ccir1" ? "CCIR-1" : "ZVEI-1"],
      ["Code", s.code],
      ["Tone duration", `${s.tone_ms} ms`],
    ]),
    body: null,
  }),

  navtex: (n) => ({
    fields: fields([
      ["Header", header(n.station, n.subject, n.serial)],
      ["Station", n.station],
      ["Subject", n.subject_name ?? n.subject],
      ["Serial", n.serial == null ? undefined : String(n.serial).padStart(2, "0")],
      ["Ended with NNNN", n.complete ? "yes" : "no — flushed early"],
      ["Repaired", n.errors_corrected > 0 ? `${n.errors_corrected} characters` : undefined],
    ]),
    body: n.text,
  }),

  acars: (a) => ({
    fields: fields([
      ["Registration", a.registration],
      ["Flight", a.flight?.trim()],
      ["Label", a.label],
      ["Mode", a.mode],
      ["Block", a.block_id],
      ["Direction", a.downlink ? "downlink" : "uplink"],
      ["Sequence", a.seq_no?.trim()],
      ["Acknowledges", a.ack ?? "NAK"],
      ["Continues", a.more ? "yes — another block follows" : undefined],
    ]),
    body: a.text || null,
  }),

  subghz: (f) => ({
    fields: fields([
      ["Model", f.reading?.model],
      ["Sensor id", f.reading == null ? undefined : hex2(f.reading.id)],
      ["Channel", f.reading?.channel == null ? undefined : String(f.reading.channel)],
      [
        "Temperature",
        f.reading?.temperature_c == null ? undefined : `${f.reading.temperature_c.toFixed(1)} °C`,
      ],
      [
        "Humidity",
        f.reading?.humidity_pct == null ? undefined : `${f.reading.humidity_pct.toFixed(0)} %`,
      ],
      [
        "Soil moisture",
        f.reading?.moisture_pct == null ? undefined : `${f.reading.moisture_pct.toFixed(0)} %`,
      ],
      [
        "Tyre pressure",
        f.reading?.pressure_kpa == null ? undefined : `${f.reading.pressure_kpa.toFixed(0)} kPa`,
      ],
      [
        "Wind average",
        f.reading?.wind_avg_kmh == null ? undefined : `${f.reading.wind_avg_kmh.toFixed(1)} km/h`,
      ],
      [
        "Wind gust",
        f.reading?.wind_max_kmh == null ? undefined : `${f.reading.wind_max_kmh.toFixed(1)} km/h`,
      ],
      [
        "Wind direction",
        f.reading?.wind_dir_deg == null ? undefined : `${f.reading.wind_dir_deg.toFixed(0)}\u00b0`,
      ],
      ["Rain", f.reading?.rain_mm == null ? undefined : `${f.reading.rain_mm.toFixed(1)} mm`],
      ["Power", f.reading?.power_w == null ? undefined : `${f.reading.power_w.toFixed(0)} W`],
      [
        "Energy",
        f.reading?.energy_kwh == null ? undefined : `${f.reading.energy_kwh.toFixed(2)} kWh`,
      ],
      ["Battery", flag(f.reading?.battery_ok)],
      ["Modulation", f.modulation],
      ["Encoding", f.encoding],
      ["Payload", f.bits === 0 ? undefined : `${f.data} (${f.bits} bit)`],
      ["EV1527 address", f.address == null ? undefined : hex5(f.address)],
      ["EV1527 button", f.button == null ? undefined : f.button.toString(16).toUpperCase()],
      ["PT2262 tri-state", f.tri_state],
      ["Base period", f.short_us > 0 ? `${f.short_us} µs` : undefined],
      ["Repeats", f.repeats > 1 ? `×${f.repeats}` : undefined],
    ]),
    body: timings(f.timings_us ?? []),
  }),

  ident: (r) => ({
    fields: [
      ["Modulation", modulationLabel(r)],
      ["Confidence", `${Math.round(r.confidence * 100)}%`],
      ...identMeasurements(r),
      ...(r.features.frequency_levels > 1
        ? ([["Frequency levels", String(r.features.frequency_levels)]] as DetailField[])
        : []),
    ],
    body:
      (r.candidates ?? []).length === 0
        ? null
        : (r.candidates ?? []).map((m) => `${m.name} — ${candidateScore(m)} — ${m.why}`).join("\n"),
  }),

  scrambler: (s) => ({
    fields: fields([
      ["Inversion", s.inversion_hz == null ? undefined : `${s.inversion_hz.toFixed(0)} Hz`],
      ["Confidence", s.inversion_hz == null ? undefined : `${(s.confidence * 100).toFixed(0)}%`],
    ]),
    body: null,
  }),

  tone: (t) => ({
    fields: fields([
      ["CTCSS", t.ctcss_hz == null ? undefined : `${t.ctcss_hz.toFixed(1)} Hz`],
      ["DCS", t.dcs_code == null ? undefined : String(t.dcs_code).padStart(3, "0")],
      ["Audio", t.open ? "open" : "muted"],
    ]),
    body: null,
  }),

  call: (c) => ({
    fields: fields([
      ["Mode", c.mode.toUpperCase()],
      [
        "Destination",
        c.destination == null
          ? undefined
          : c.group_call === false
            ? `radio ${c.destination}`
            : `talkgroup ${c.destination}`,
      ],
      ["Source", c.source == null ? undefined : String(c.source)],
      ["Timeslot", c.slot == null ? undefined : String(c.slot)],
      ["Colour code", c.color_code == null ? undefined : String(c.color_code)],
      ["Duration", `${(c.duration_ms / 1000).toFixed(1)} s`],
      ["Started", c.started_at],
      ["Emergency", c.emergency ? "yes" : undefined],
      ["Encrypted", c.encrypted ? "yes" : undefined],
      ["Audio", c.audio_error ?? undefined],
    ]),
    body: null,
  }),
  dv: (f) => ({
    fields: fields([
      ["Mode", dvMode(f)],
      ["Frame", dvKind(f)],
      ["Network", dvNetwork(f)],
      ["Vendor", dvVendor(f)],
      ["Trunking", dvTrunking(f)],
      ["Parties", dvParties(f)],
      ["Talker alias", f.talker_alias],
      ["Call", f.group_call == null ? undefined : f.group_call ? "talkgroup" : "private"],
      ["Via", f.via],
      ["Signalling", f.opcode],
      ["Position", position(f.lat, f.lon)],
      ["Position error", f.position_error_m == null ? undefined : `≤ ${f.position_error_m} m`],
      ["Channel", f.channel == null ? undefined : String(f.channel)],
      [
        "Channel frequency",
        f.channel_definition == null
          ? undefined
          : `LCN ${f.channel_definition.channel} · TX ${(f.channel_definition.tx_hz / 1e6).toFixed(6)} MHz · RX ${(f.channel_definition.rx_hz / 1e6).toFixed(6)} MHz`,
      ],
      ["Rest channel", f.rest_channel == null ? undefined : String(f.rest_channel)],
      ["Network ID", f.network_id == null ? undefined : String(f.network_id)],
      ["System ID", f.system_id == null ? undefined : String(f.system_id)],
      ["Site ID", f.site_id == null ? undefined : String(f.site_id)],
      ["Emergency", flag(f.emergency)],
      ["Late entry", flag(f.late_entry)],
      ["Slot activity", slotActivity(f)],
      ["Encrypted", flag(f.encrypted)],
      ["Algorithm", f.algorithm_id == null ? undefined : hex(f.algorithm_id, 2)],
      ["Key ID", f.key_id == null ? undefined : hex(f.key_id, 4)],
      ["Message indicator", f.message_indicator],
      ["Repaired", f.errors_corrected > 0 ? `${f.errors_corrected} bits` : undefined],
      ["Checksum", dvChecksum(f)],
    ]),
    body: f.text ?? f.data ?? null,
  }),

  broadcast: (status) => ({
    fields: fields([
      ["System", broadcastSystem(status.system)],
      ["Lock", status.locked ? "locked" : "searching"],
      ["SNR", status.locked ? `${status.snr_db.toFixed(1)} dB` : undefined],
      [
        "Frequency error",
        status.locked
          ? `${status.frequency_error_hz >= 0 ? "+" : ""}${status.frequency_error_hz.toFixed(0)} Hz`
          : undefined,
      ],
      ["Symbol rate", status.symbol_rate == null ? undefined : `${status.symbol_rate} Bd`],
      ["Ensemble ID", status.ensemble_id == null ? undefined : hex(status.ensemble_id, 4)],
      ["Service ID", status.service_id == null ? undefined : hex(status.service_id, 4)],
      ["Label", status.label],
    ]),
    body: null,
  }),
  radio_clock: (r) => ({
    fields: fields([
      ["Service", r.standard.toUpperCase()],
      ["Civil time", r.datetime],
      ["UTC offset", utcOffset(r.utc_offset_minutes)],
      ["Daylight saving", r.dst ? "active" : "inactive"],
      ["Leap warning", r.leap_warning ? "yes" : undefined],
      ["DUT1", r.dut1_seconds == null ? undefined : `${r.dut1_seconds.toFixed(1)} s`],
    ]),
    body: r.symbols || null,
  }),

  gnss: (g) => ({
    fields: fields([
      ["Signal", `GPS L1 C/A PRN ${g.prn}`],
      ["Doppler", `${g.doppler_hz.toFixed(0)} Hz`],
      ["Code phase", `${g.code_phase_chips.toFixed(2)} chips`],
      ["C/N₀", `${g.cn0_db_hz.toFixed(1)} dB-Hz`],
      ["NAV subframe", g.subframe == null ? "acquiring telemetry" : String(g.subframe)],
      ["Time of week", g.tow_seconds == null ? undefined : `${g.tow_seconds} s`],
      ["GPS week (10 bit)", g.week == null ? undefined : String(g.week)],
    ]),
    body: (g.words ?? []).join(" ") || null,
  }),
  sstv: (p) => ({
    fields: fields([
      ["Mode", SSTV_MODE_LABELS[p.mode]],
      ["Size", `${p.width} \u00d7 ${p.height}`],
      ["Lines received", `${p.lines} of ${p.height}`],
      ["State", p.complete ? "complete" : "cut short"],
      ["Took", `${(p.duration_ms / 1000).toFixed(1)} s`],
    ]),
    body: null,
  }),
  vor: (v) => ({
    fields: fields([
      ["Station", v.station],
      ["Radial", `${v.radial_deg.toFixed(2)}°`],
      ["Variable phase", `${v.variable_phase_deg.toFixed(2)}°`],
      ["Reference phase", `${v.reference_phase_deg.toFixed(2)}°`],
      ["Magnetic declination", `${signed(v.magnetic_declination_deg, 1)}°`],
      ["Station position", position(v.station_lat, v.station_lon)],
      ["Signal", `${v.signal_db.toFixed(1)} dB`],
      ["Confidence", `${Math.round(v.confidence * 100)}%`],
    ]),
    body: null,
  }),
  df: (b) => ({
    fields: fields([
      ["Station", b.station_id],
      ["Bearing", `${b.bearing_deg.toFixed(2)}°`],
      ["Confidence", `${Math.round(b.confidence * 100)}%`],
      ["Seen from", position(b.lat, b.lon)],
    ]),
    body: null,
  }),
  df_fix: (e) => ({
    fields: fields([
      ["Position", position(e.lat, e.lon)],
      ["Uncertainty", `${Math.round(e.ellipse_major_m)} × ${Math.round(e.ellipse_minor_m)} m`],
      ["Ellipse bearing", `${e.ellipse_bearing_deg.toFixed(1)}°`],
      ["Bearings used", String(e.samples)],
    ]),
    body: null,
  }),
  radar: (d) => ({
    fields: fields([
      ["Range bin", String(d.range_bin)],
      ["Bistatic range", `${d.range_km.toFixed(2)} km`],
      ["Doppler", `${signed(d.doppler_hz, 1)} Hz`],
      ["SNR", `${d.snr_db.toFixed(1)} dB`],
    ]),
    body: null,
  }),
  ils: (i) => ({
    fields: fields([
      ["Component", i.component === "localizer" ? "localizer" : "glideslope"],
      ["90 Hz modulation", `${(i.modulation_90 * 100).toFixed(2)}%`],
      ["150 Hz modulation", `${(i.modulation_150 * 100).toFixed(2)}%`],
      ["DDM", signed(i.ddm, 4)],
      ["Deviation", `${signed(i.deviation_dots, 2)} dots`],
      ["Signal", `${i.signal_db.toFixed(1)} dB`],
    ]),
    body: null,
  }),
  dsc: dataLinkDetail,
  inmarsat_stdc: dataLinkDetail,
  inmarsat_aero: dataLinkDetail,
  vdl2: dataLinkDetail,
  hfdl: dataLinkDetail,
  iridium: dataLinkDetail,
  dect: dectDetail,
};

function dataLinkDetail(message: DataLinkMessage): EventDetail {
  const serialized = JSON.stringify(message.details, null, 2);
  const detail =
    serialized == null || serialized === "{}" || serialized === "null" ? null : serialized;
  return {
    fields: fields([
      ["Message type", message.message_type],
      ["Station", message.station],
      ["Integrity", message.crc_ok ? "verified" : "failed"],
      ["FEC repaired", message.fec_corrected == null ? undefined : String(message.fec_corrected)],
      ["SNR", message.snr_db == null ? undefined : `${message.snr_db.toFixed(1)} dB`],
      [
        "Frequency error",
        message.frequency_error_hz == null
          ? undefined
          : `${signed(message.frequency_error_hz, 1)} Hz`,
      ],
      ["Position", position(message.lat, message.lon)],
      ["Raw", message.raw],
    ]),
    body:
      [message.text?.trim() || null, detail].filter((value) => value !== null).join("\n\n") || null,
  };
}

function utcOffset(minutes: number | null | undefined): string | undefined {
  if (minutes == null) return undefined;
  const sign = minutes < 0 ? "−" : "+";
  const absolute = Math.abs(minutes);
  return `UTC${sign}${String(Math.floor(absolute / 60)).padStart(2, "0")}:${String(absolute % 60).padStart(2, "0")}`;
}

function broadcastSystem(system: string): string {
  const labels: Record<string, string> = {
    dab: "DAB",
    dab_plus: "DAB+",
    dvb_s: "DVB-S",
    dvb_s2: "DVB-S2",
    drm30: "DRM30",
    drm_plus: "DRM+",
  };
  return labels[system] ?? system;
}

function dvVendor(frame: DvFrame): string | undefined {
  if (frame.vendor == null) return undefined;
  const names: Record<NonNullable<DvFrame["vendor"]>, string> = {
    standard: "standard",
    etsi: "ETSI",
    motorola: "Motorola",
    hytera: "Hytera",
    harris: "Harris",
    tait: "Tait",
    jvc_kenwood: "JVCKENWOOD",
    emc: "EMC",
    radio_activity: "Radio Activity",
    flyde_micro: "Flyde Micro",
    prod_el: "PROD-EL",
    unknown: "unknown vendor",
  };
  const mfid = frame.manufacturer_id == null ? "" : ` (${hex(frame.manufacturer_id, 2)})`;
  return `${names[frame.vendor]}${mfid}`;
}

function slotActivity(frame: DvFrame): string | undefined {
  if (frame.slot_activity == null || frame.slot_activity.length === 0) return undefined;
  return frame.slot_activity
    .map(
      (item) =>
        `TS${item.slot} ${item.activity}${
          item.destination_hash == null ? "" : ` (hash ${hex(item.destination_hash, 2)})`
        }`,
    )
    .join(", ");
}

function hex(value: number, width: number): string {
  return `0x${value.toString(16).toUpperCase().padStart(width, "0")}`;
}

function dvKind(frame: Pick<DvFrame, "kind">): string {
  switch (frame.kind) {
    case "header":
      return "call";
    case "voice":
      return "in progress";
    case "terminator":
      return "end";
    case "control":
      return "signalling";
    case "data":
      return "data";
  }
}

function monitorLine(packet: AprsPacket): string | null {
  return packet.tnc2 || null;
}

const DECT_CLASS_LABELS: Record<string, string> = {
  a: "A residential / small PBX",
  b: "B private multi-cell",
  c: "C public access",
  d: "D public GSM/UMTS",
  e: "E PP-to-PP direct",
  f: "F reserved",
  g: "G reserved",
  h: "H reserved",
};

const DECT_CAPABILITY_LABELS: Record<string, string> = {
  extended_fp_info: "extended FP info",
  double_duplex_bearer: "double duplex bearer",
  double_slot: "double slot",
  half_slot: "half slot",
  full_slot: "full slot",
  frequency_control: "frequency control",
  page_repetition: "page repetition",
  co_setup_on_dummy: "C/O setup on dummy",
  cl_uplink: "C/L uplink",
  cl_downlink: "C/L downlink",
  basic_a_field_setup: "basic A-field setup",
  advanced_a_field_setup: "advanced A-field setup",
  b_field_setup: "B-field setup",
  cf_messages: "Cf messages",
  in_minimum_delay: "IN minimum delay",
  in_normal_delay: "IN normal delay",
  ip_error_detection: "IP error detection",
  ip_error_correction: "IP error correction",
  multibearer_connections: "multibearer connections",
  adpcm: "ADPCM/G.726",
  gap_basic_speech: "GAP basic speech",
  non_voice_circuit_switched: "non-voice circuit switched",
  non_voice_packet_switched: "non-voice packet switched",
  standard_authentication: "standard authentication (DSAA)",
  standard_ciphering: "standard ciphering (DSC)",
  location_registration: "location registration",
  sim_services: "SIM services",
  non_static_fixed_part: "non-static fixed part",
  ciss_services: "CISS services",
  clms_service: "CLMS service",
  coms_service: "COMS service",
  access_rights_requests: "access rights requests",
  external_handover: "external handover",
  connection_handover: "connection handover",
};

export function dectCarriers(mask: number | null | undefined): string | undefined {
  if (mask == null) return undefined;
  const on = [];
  for (let carrier = 0; carrier < 10; carrier += 1) {
    if ((mask >> (9 - carrier)) & 1) on.push(carrier);
  }
  return on.length === 0 ? undefined : on.join(", ");
}

function dectDetail(frame: DectFrame): EventDetail {
  const id = frame.identity;
  const capabilities = (frame.capabilities ?? []).map(
    (capability) => DECT_CAPABILITY_LABELS[capability] ?? capability,
  );
  return {
    fields: fields([
      ["Side", frame.side === "rfp" ? "base station" : "handset"],
      ["RFPI", id?.rfpi],
      ["Access rights class", id ? (DECT_CLASS_LABELS[id.arc] ?? id.arc) : undefined],
      ["PARI", id?.pari],
      ["Manufacturer code", id?.emc == null ? undefined : hex4(id.emc)],
      ["Installer code", id?.eic == null ? undefined : hex4(id.eic)],
      ["Operator code", id?.poc == null ? undefined : hex4(id.poc)],
      ["GSM/UMTS operator", id?.gop == null ? undefined : hex5(id.gop)],
      ["Fixed part number", id?.fpn == null ? undefined : String(id.fpn)],
      ["Fixed part sub-number", id?.fps == null ? undefined : String(id.fps)],
      ["Radio fixed part", id == null ? undefined : String(id.rpn)],
      ["Cell", id?.multicell == null ? undefined : id.multicell ? "multi-cell" : "single cell"],
      ["SARI list", flag(id?.sari_available)],
      ["Carrier", frame.carrier == null ? undefined : String(frame.carrier)],
      ["Frequency", frame.carrier_hz == null ? undefined : mhz(frame.carrier_hz)],
      ["Slot pair", frame.slot_pair == null ? undefined : String(frame.slot_pair)],
      ["Transceivers", frame.transceivers == null ? undefined : String(frame.transceivers)],
      ["Carriers available", dectCarriers(frame.rf_carriers)],
      ["Scan carrier", frame.pscn == null ? undefined : String(frame.pscn)],
      ["Multiframe", frame.multiframe == null ? undefined : String(frame.multiframe)],
      ["Authentication", flag(frame.security.authentication_supported)],
      ["Ciphering", flag(frame.security.ciphering_supported)],
      [
        "Encryption",
        DECT_CIPHER_LABELS[frame.security.cipher_state] ?? frame.security.cipher_state,
      ],
      ["Last cipher command", frame.security.last_command],
      [
        "Cipher key index",
        frame.security.cipher_key_index == null
          ? undefined
          : String(frame.security.cipher_key_index),
      ],
      ["FMID", frame.fmid == null ? undefined : hex4(frame.fmid)],
      ["PMID", frame.pmid == null ? undefined : hex5(frame.pmid)],
      ["Handsets", (frame.handsets ?? []).map(hex5).join(", ")],
      ["Bursts", String(frame.bursts)],
      ["A-field CRC errors", String(frame.crc_errors)],
      ["Level", `${frame.level_dbfs.toFixed(1)} dBFS`],
    ]),
    body: capabilities.length === 0 ? null : capabilities.join("\n"),
  };
}

function hex4(value: number): string {
  return value.toString(16).toUpperCase().padStart(4, "0");
}

function fields(rows: readonly (readonly [string, string | null | undefined])[]): DetailField[] {
  return rows.flatMap(([label, value]) =>
    value == null || value === "" ? [] : [[label, value] as DetailField],
  );
}

function signed(value: number, digits: number): string {
  return `${value >= 0 ? "+" : ""}${value.toFixed(digits)}`;
}

function flag(value: boolean | null | undefined): string | undefined {
  return value == null ? undefined : value ? "yes" : "no";
}

function feet(ft: number | null | undefined): string | undefined {
  return ft == null ? undefined : `${ft.toLocaleString("en-US")} ft`;
}

function knots(kt: number | null | undefined): string | undefined {
  return kt == null ? undefined : `${kt.toFixed(1)} kt`;
}

function degrees(deg: number | null | undefined): string | undefined {
  return deg == null ? undefined : `${Math.round(deg)}°`;
}

function position(
  lat: number | null | undefined,
  lon: number | null | undefined,
): string | undefined {
  return lat == null || lon == null ? undefined : `${lat.toFixed(5)}, ${lon.toFixed(5)}`;
}

function mhz(hz: number): string {
  return `${(hz / 1e6).toFixed(1)} MHz`;
}

function header(
  station: string | null | undefined,
  subject: string | null | undefined,
  serial: number | null | undefined,
): string | undefined {
  if (station == null || subject == null || serial == null) {
    return undefined;
  }
  return `${station}${subject}${String(serial).padStart(2, "0")}`;
}

function timings(us: readonly number[]): string | null {
  if (us.length === 0) {
    return null;
  }
  const pairs: string[] = [];
  for (let i = 0; i < us.length; i += 2) {
    const gap = us[i + 1];
    pairs.push(gap == null ? `${us[i]}` : `${us[i]}/${gap}`);
  }
  return pairs.join("  ");
}
