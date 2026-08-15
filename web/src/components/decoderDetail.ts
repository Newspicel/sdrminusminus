// Everything a decoded frame carried, for the row the decoder log expands.
//
// A log row is one line, and one line cannot hold a NAVTEX broadcast's text, an ACARS block's
// body or a sub-GHz burst's pulse train. Those used to be read in per-channel panes, which were
// a second copy of this log; the detail lives here instead, so there is one place a frame is read
// and it is the place that stored it.
//
// Pure and exhaustive over `DecoderKind`: a decoder added to `wire` fails the typecheck here
// until its fields have somewhere to be shown.

import type { AprsPacket, DecoderEvent, DecoderKind, DvFrame } from "../lib/types";
import { hex5 } from "./decoderLog";
import {
  candidateScore,
  dvMode,
  dvNetwork,
  dvParties,
  identMeasurements,
  modulationLabel,
} from "./decoderViews";

/** One `label: value` pair. Only the fields a frame actually carried are emitted — an absent
 * field is a thing the transmission did not say, and a column of em dashes says it worse. */
export type DetailField = readonly [label: string, value: string];

export interface EventDetail {
  fields: DetailField[];
  /** Free text that needs its own block: a broadcast, a message body, a pulse train. `null`
   * when the frame carried none, which is most of them. */
  body: string | null;
}

/** Every field of a decoded frame, laid out for reading. */
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
      // A broadcast the carrier cut short is still worth reading, but not worth trusting
      // whole, and the row is the only place that distinction survives.
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
      ["Modulation", f.modulation],
      ["Encoding", f.encoding],
      ["Payload", f.bits === 0 ? undefined : `${f.data} (${f.bits} bit)`],
      ["EV1527 address", f.address == null ? undefined : hex5(f.address)],
      ["EV1527 button", f.button == null ? undefined : f.button.toString(16).toUpperCase()],
      ["PT2262 tri-state", f.tri_state],
      ["Base period", f.short_us > 0 ? `${f.short_us} µs` : undefined],
      ["Repeats", f.repeats > 1 ? `×${f.repeats}` : undefined],
    ]),
    // Pulse first, then gap, as the decoder measured them. Truncated at the source, so this is
    // for inspection and never for replay.
    body: timings(f.timings_us ?? []),
  }),

  // The measurements first, then the shortlist. A verdict with nothing behind it is worth less
  // than no verdict, so the numbers it was decided from are part of the answer rather than a
  // debugging aside.
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

  tone: (t) => ({
    fields: fields([
      ["CTCSS", t.ctcss_hz == null ? undefined : `${t.ctcss_hz.toFixed(1)} Hz`],
      ["DCS", t.dcs_code == null ? undefined : String(t.dcs_code).padStart(3, "0")],
      ["Audio", t.open ? "open" : "muted"],
    ]),
    body: null,
  }),

  dv: (f) => ({
    fields: fields([
      ["Mode", dvMode(f)],
      ["Frame", dvKind(f)],
      ["Network", dvNetwork(f)],
      ["Vendor", dvVendor(f)],
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
      // `Some(false)` is a positive statement that the payload is in the clear; absent means the
      // frame did not say, which is not the same thing.
      ["Encrypted", flag(f.encrypted)],
      ["Algorithm", f.algorithm_id == null ? undefined : hex(f.algorithm_id, 2)],
      ["Key ID", f.key_id == null ? undefined : hex(f.key_id, 4)],
      ["Message indicator", f.message_indicator],
      ["Repaired", f.errors_corrected > 0 ? `${f.errors_corrected} bits` : undefined],
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
};

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

/** What the frame was: the words a scanner shows rather than the specification's field names. */
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

/** A Mic-E monitor line is packed binary, so it is shown as the body rather than pretended to be
 * readable inline; every other packet's line already reads as text. */
function monitorLine(packet: AprsPacket): string | null {
  return packet.tnc2 || null;
}

/** Drops the fields the frame did not carry, so the list is what was received and not a form. */
function fields(rows: readonly (readonly [string, string | null | undefined])[]): DetailField[] {
  return rows.flatMap(([label, value]) =>
    value == null || value === "" ? [] : [[label, value] as DetailField],
  );
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

/** Pulse/gap durations as pairs, so the reader can see the cell structure rather than a wall of
 * numbers. */
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
