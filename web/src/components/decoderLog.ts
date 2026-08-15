import type { DecodedState } from "../lib/decoded";
import type {
  DecodedRecord,
  DecoderEvent,
  DecoderKind,
  DecoderLogEntry,
  DecoderLogFilter,
} from "../lib/types";
import { candidateScore, dvMode, dvNetwork, dvParties, modulationLabel } from "./decoderViews";

export const KIND_LABELS: Record<DecoderKind, string> = {
  adsb: "ADS-B",
  ais: "AIS",
  aprs: "APRS",
  pocsag: "POCSAG",
  rds: "RDS",
  rtty: "RTTY",
  morse: "Morse",
  selcall: "Selcall",
  navtex: "NAVTEX",
  acars: "ACARS",
  subghz: "Sub-GHz",
  tone: "Tone",
  dv: "Digital voice",
  ident: "Signal ID",
  ft8: "FT8",
  ft4: "FT4",
  psk31: "PSK31",
  psk63: "PSK63",
  wspr: "WSPR",
  broadcast: "Digital broadcast",
  radio_clock: "Radio clock",
  gnss: "GNSS lab",
};

export const DECODER_KINDS = Object.keys(KIND_LABELS) as DecoderKind[];

export const LIMIT_OPTIONS = [100, 500, 2000];

export const LIVE_ROW_CAP = 200;

export interface LogFilter {
  q: string;
  limit: number;
}

export const DEFAULT_LOG_FILTER: LogFilter = { q: "", limit: 500 };

export interface LogRow {
  key: string;
  at: string;
  kind: string;
  station: string | null;
  summary: string;
  freqHz: number;
  deviceSet: number;
  channel: number;
  live: boolean;
  event: DecoderEvent;
}

export function kindLabel(kind: string): string {
  return KIND_LABELS[kind as DecoderKind] ?? kind.toUpperCase();
}

export interface WireScope {
  nodes: string;
  sources: string;
}

export const NO_WIRES: WireScope = { nodes: "", sources: "" };

export function sourceSet(sources: string): ReadonlySet<string> {
  return new Set(sources === "" ? [] : sources.split(","));
}

function inSources(record: DecodedRecord, sources: ReadonlySet<string>): boolean {
  return sources.has(`${record.device_set}:${record.channel}`);
}

export function toQuery(filter: LogFilter, wires: WireScope): DecoderLogFilter {
  const query: DecoderLogFilter = {
    limit: filter.limit,
    nodes: wires.nodes,
    sources: wires.sources,
  };
  const q = filter.q.trim();
  if (q !== "") {
    query.q = q;
  }
  return query;
}

export function isFiltered(filter: LogFilter): boolean {
  return filter.q.trim() !== "";
}

export function matchesFilter(
  record: DecodedRecord,
  filter: LogFilter,
  sources: ReadonlySet<string>,
): boolean {
  if (!inSources(record, sources)) {
    return false;
  }
  const q = filter.q.trim().toLowerCase();
  if (q === "") {
    return true;
  }
  const station = eventStation(record.event);
  return (
    eventSummary(record.event).toLowerCase().includes(q) ||
    (station !== null && station.toLowerCase().includes(q))
  );
}

export function collectLive(
  frames: DecodedState["frames"],
  filter: LogFilter,
  sources: ReadonlySet<string>,
  cap = LIVE_ROW_CAP,
): DecodedRecord[] {
  const records: DecodedRecord[] = [];
  for (const slice of Object.values(frames)) {
    for (const record of slice ?? []) {
      if (matchesFilter(record, filter, sources)) {
        records.push(record);
      }
    }
  }
  records.sort((a, b) => timeMs(b.at) - timeMs(a.at));
  return records.length > cap ? records.slice(0, cap) : records;
}

export function buildRows(
  entries: readonly DecoderLogEntry[],
  live: readonly DecodedRecord[],
): LogRow[] {
  const stored = entries.map(storedRow);
  const seen = new Set(stored.map(signature));
  const rows: LogRow[] = [];
  for (const record of live) {
    const row = liveRow(record);
    const key = signature(row);
    if (!seen.has(key)) {
      seen.add(key);
      rows.push(row);
    }
  }
  rows.push(...stored);
  rows.sort((a, b) => timeMs(b.at) - timeMs(a.at));
  return rows;
}

export function storedRow(entry: DecoderLogEntry): LogRow {
  return {
    key: `stored:${entry.id}`,
    at: entry.at,
    kind: entry.kind,
    station: entry.station ?? null,
    summary: entry.summary,
    freqHz: entry.freq_hz,
    deviceSet: entry.device_set,
    channel: entry.channel,
    live: false,
    event: entry.event,
  };
}

export function liveRow(record: DecodedRecord): LogRow {
  const row: LogRow = {
    key: "",
    at: record.at,
    kind: record.event.kind,
    station: eventStation(record.event),
    summary: eventSummary(record.event),
    freqHz: record.freq_hz,
    deviceSet: record.device_set,
    channel: record.channel,
    live: true,
    event: record.event,
  };
  row.key = `live:${signature(row)}`;
  return row;
}

export function eventSummary(event: DecoderEvent): string {
  switch (event.kind) {
    case "rds": {
      const r = event.data;
      return join([
        r.pi == null ? null : `PI ${r.pi}`,
        r.ps ?? null,
        r.pty_name ?? null,
        r.radiotext ?? null,
      ]);
    }
    case "pocsag": {
      const p = event.data;
      return p.text === "" ? `${p.address} (${p.function})` : `${p.address}: ${p.text}`;
    }
    case "adsb": {
      const a = event.data;
      return join([
        a.icao,
        a.callsign?.trim() ?? null,
        a.altitude_ft == null ? null : `${a.altitude_ft} ft`,
        position(a.lat, a.lon),
      ]);
    }
    case "ais": {
      const m = event.data;
      return join([String(m.mmsi), m.name?.trim() ?? null, position(m.lat, m.lon)]);
    }
    case "aprs": {
      const p = event.data;
      return p.mic_e_message == null ? p.tnc2 : join([p.tnc2, p.mic_e_message]);
    }
    case "rtty":
    case "morse":
    case "psk31":
    case "psk63":
      return event.data.text;
    case "ft8":
    case "ft4": {
      const message = event.data;
      return join([
        message.text,
        `${message.snr_db >= 0 ? "+" : ""}${message.snr_db.toFixed(0)} dB`,
        `${message.audio_hz.toFixed(0)} Hz`,
      ]);
    }
    case "wspr": {
      const spot = event.data;
      return join([
        spot.text,
        `${spot.snr_db >= 0 ? "+" : ""}${spot.snr_db.toFixed(0)} dB`,
        `${spot.audio_hz.toFixed(0)} Hz`,
      ]);
    }
    case "selcall":
      return `${event.data.system === "ccir1" ? "CCIR-1" : "ZVEI-1"} · ${event.data.code}`;
    case "navtex": {
      const n = event.data;
      const header =
        n.station == null || n.subject == null || n.serial == null
          ? null
          : `${n.station}${n.subject}${String(n.serial).padStart(2, "0")}`;
      return join([header, n.subject_name ?? null, n.text.replaceAll("\n", " ").trim()]);
    }
    case "acars": {
      const a = event.data;
      const text = a.text.replaceAll("\n", " ").trim();
      return join([a.registration, a.flight?.trim() ?? null, `[${a.label}]`, text || null]);
    }
    case "subghz": {
      const f = event.data;
      return join([
        f.bits === 0 ? `raw, ${(f.timings_us ?? []).length} edges` : `${f.bits} bit ${f.data}`,
        f.address == null ? null : `addr ${hex5(f.address)}`,
        f.button == null ? null : `btn ${f.button.toString(16).toUpperCase()}`,
        f.repeats > 1 ? `\u00d7${f.repeats}` : null,
      ]);
    }
    case "tone": {
      const t = event.data;
      const heard = join([
        t.ctcss_hz == null ? null : `CTCSS ${t.ctcss_hz.toFixed(1)} Hz`,
        t.dcs_code == null ? null : `DCS ${String(t.dcs_code).padStart(3, "0")}`,
      ]);
      return join([heard === "" ? "no tone" : heard, t.open ? "open" : "muted"]);
    }
    case "dv": {
      const f = event.data;
      return join([
        dvMode(f),
        dvNetwork(f) || null,
        dvParties(f) || null,
        f.via == null ? null : `via ${f.via}`,
        f.opcode ?? null,
        f.encrypted === true ? "encrypted" : null,
        f.text ?? null,
      ]);
    }
    case "ident": {
      const r = event.data;
      const best = r.candidates?.[0];
      return join([
        modulationLabel(r),
        r.modulation === "none" ? null : `${(r.bandwidth_hz / 1000).toFixed(1)} kHz`,
        r.symbol_rate_hz == null ? null : `${Math.round(r.symbol_rate_hz)} Bd`,
        r.deviation_hz == null ? null : `\u00b1${Math.round(r.deviation_hz)} Hz`,
        best == null ? null : `${best.name} (${candidateScore(best)})`,
      ]);
    }
    case "broadcast": {
      const status = event.data;
      return join([
        broadcastSystem(status.system),
        status.locked ? "locked" : "searching",
        status.locked ? `${status.snr_db.toFixed(1)} dB SNR` : null,
        status.locked
          ? `${status.frequency_error_hz >= 0 ? "+" : ""}${status.frequency_error_hz.toFixed(0)} Hz`
          : null,
        status.label ?? null,
      ]);
    }
    case "radio_clock": {
      const r = event.data;
      return join([r.standard.toUpperCase(), r.datetime, r.leap_warning ? "leap warning" : null]);
    }
    case "gnss": {
      const g = event.data;
      return join([
        `GPS PRN ${g.prn}`,
        `${g.doppler_hz >= 0 ? "+" : ""}${g.doppler_hz.toFixed(0)} Hz`,
        `${g.cn0_db_hz.toFixed(1)} dB-Hz`,
        g.subframe == null ? "acquired" : `subframe ${g.subframe}`,
        g.tow_seconds == null ? null : `TOW ${g.tow_seconds} s`,
      ]);
    }
  }
}

export function eventStation(event: DecoderEvent): string | null {
  switch (event.kind) {
    case "adsb":
      return event.data.icao;
    case "ais":
      return String(event.data.mmsi);
    case "aprs":
      return event.data.source;
    case "pocsag":
      return String(event.data.address);
    case "rds":
      return event.data.pi ?? null;
    case "navtex":
      return event.data.station ?? null;
    case "acars":
      return event.data.registration;
    case "subghz": {
      const f = event.data;
      if (f.address != null) {
        return hex5(f.address);
      }
      return f.data === "" ? null : f.data;
    }
    case "dv":
      return (
        event.data.source_call ?? (event.data.source == null ? null : String(event.data.source))
      );
    case "ft8":
    case "ft4":
      return event.data.text.split(/\s+/)[1] ?? null;
    case "wspr":
      return event.data.callsign;
    case "radio_clock":
      return event.data.standard.toUpperCase();
    case "gnss":
      return `GPS-${event.data.prn}`;
    case "rtty":
    case "morse":
    case "psk31":
    case "psk63":
    case "selcall":
    case "tone":
    case "ident":
      return null;
    case "broadcast": {
      const status = event.data;
      const id = status.service_id ?? status.ensemble_id;
      return id == null ? null : id.toString(16).toUpperCase();
    }
  }
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

export function hex5(address: number): string {
  return address.toString(16).toUpperCase().padStart(5, "0");
}

export function droppedNotice(lost: number, dropped: number): string | null {
  if (lost <= 0 && dropped <= 0) {
    return null;
  }
  const parts: string[] = [];
  if (lost > 0) {
    parts.push(`${lost} live ${frameWord(lost)} dropped`);
  }
  if (dropped > 0) {
    parts.push(`${dropped} ${frameWord(dropped)} never reached the log`);
  }
  return parts.join(" · ");
}

function frameWord(n: number): string {
  return n === 1 ? "frame" : "frames";
}

function signature(row: LogRow): string {
  return `${row.at}|${row.kind}|${row.deviceSet}|${row.channel}|${row.station ?? ""}|${row.summary}`;
}

function join(parts: readonly (string | null)[]): string {
  return parts.filter((p) => p !== null).join(" · ");
}

function position(lat: number | null | undefined, lon: number | null | undefined): string | null {
  return lat == null || lon == null ? null : `${lat.toFixed(4)}, ${lon.toFixed(4)}`;
}

function timeMs(at: string): number {
  const ms = Date.parse(at);
  return Number.isNaN(ms) ? 0 : ms;
}
