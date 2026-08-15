// Row model and filter plumbing for the decoder log (: the log is queryable and
// exportable, not scroll-back-only). The panel renders two sources through one row shape: the
// stored page from `GET /api/decoderlog`, and the live tail from the WS store. Everything here
// is pure so the panel stays a rendering shell.

import type { DecodedState } from "../lib/decoded";
import type {
  DecodedRecord,
  DecoderEvent,
  DecoderKind,
  DecoderLogEntry,
  DecoderLogFilter,
} from "../lib/types";
import { dvMode, dvNetwork, dvParties } from "./decoderViews";

/** Exhaustive over `DecoderKind`: adding a decoder to `wire` fails the typecheck here until it
 * gets a label, and `DECODER_KINDS` follows for free. */
export const KIND_LABELS: Record<DecoderKind, string> = {
  adsb: "ADS-B",
  ais: "AIS",
  aprs: "APRS",
  pocsag: "POCSAG",
  rds: "RDS",
  rtty: "RTTY",
  morse: "Morse",
  navtex: "NAVTEX",
  acars: "ACARS",
  subghz: "Sub-GHz",
  tone: "Tone",
  dv: "Digital voice",
};

export const DECODER_KINDS = Object.keys(KIND_LABELS) as DecoderKind[];

export const LIMIT_OPTIONS = [100, 500, 2000];

/** Live rows rendered above the fetched page. A decoder can run at hundreds of frames a second;
 * the tail is a "what is happening now" readout, not a second copy of the log. */
export const LIVE_ROW_CAP = 200;

/**
 * What the operator narrows *within* the node's own scope. Which decoders and which radios a log
 * shows is the wires' answer, not a dropdown's — a log node shows what is plugged into it, and a
 * picker that could contradict the patch is a second place to read the same fact.
 */
export interface LogFilter {
  q: string;
  limit: number;
}

export const DEFAULT_LOG_FILTER: LogFilter = { q: "", limit: 500 };

/** A stored entry and a live frame, reduced to what the table draws. `live` marks rows that
 * exist only in this browser's tail — they are not (yet) a query the server would answer with. */
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
  /** The frame itself, for the detail the row expands to. Stored entries carry it back from the
   * database, so a row read an hour later opens on the same fields a live one does. */
  event: DecoderEvent;
}

export function kindLabel(kind: string): string {
  return KIND_LABELS[kind as DecoderKind] ?? kind.toUpperCase();
}

/**
 * The channels a decoder-log or export node is wired to, in both the names a stored row answers
 * to: the patch node ids, which are durable across runs, and the engine coordinates those nodes
 * hold right now, which are all a row written before the log recorded a node has.
 *
 * Both empty is a node with nothing wired in, and matches nothing — the same reading the server
 * gives it, which is what keeps a wire-scoped Clear from emptying the whole log.
 */
export interface WireScope {
  /** Comma-separated `PatchNode.id`s. */
  nodes: string;
  /** Comma-separated `device_set:channel` pairs. */
  sources: string;
}

export const NO_WIRES: WireScope = { nodes: "", sources: "" };

/** The wired channels as a lookup, for filtering the live tail. Live frames carry no node — they
 * are this run by definition — so the tail matches on the coordinates alone. */
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

/** Whether anything but the row limit is narrowing the view — an empty result means "nothing
 * logged" or "nothing matched", and only the filter tells the two apart. */
export function isFiltered(filter: LogFilter): boolean {
  return filter.q.trim() !== "";
}

/** The filter the server applies, re-applied to the live tail — otherwise a log node would tail
 * every decoder in the workspace rather than the ones wired into it. */
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

/** Newest-first across every decoder. Per-kind slices are each newest first already, so the
 * merge only has to order the heads — a sort is cheap at `LIVE_ROW_CAP` scale and keeps the
 * store's shape opaque here. */
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

/**
 * The tail and the stored page as one table, newest first.
 *
 * Sorted across both rather than stacked as two blocks: the writer flushes twice a second, so a
 * row crosses from the tail to the page while the operator is reading it, and a merge that only
 * ordered within each half would let that crossing reorder the table around it.
 *
 * A live frame is also persisted server-side, so a refetch returns rows the tail already shows;
 * those duplicates are dropped in favour of the stored row, which carries the real id. The sort
 * is stable, so rows sharing a timestamp keep the server's `(at, id)` order.
 */
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

/**
 * One-line summary of a live frame.
 *
 * Stored rows carry the server's `summary`; live frames arrive as raw `DecodedRecord`s and must
 * read identically in the same table, so this mirrors `DecoderEvent::summary` in
 * `crates/wire/src/decode.rs` field for field. Change one, change the other.
 */
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
      // A Mic-E monitor line is packed binary, so the message named beside it is the only
      // part of the row a reader can act on.
      return p.mic_e_message == null ? p.tnc2 : join([p.tnc2, p.mic_e_message]);
    }
    case "rtty":
    case "morse":
      return event.data.text;
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
  }
}

/** `null` for the decoders whose output is a character stream: RTTY and Morse name no emitter. */
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
    case "rtty":
    case "morse":
    // Subaudible signalling names the channel's state, not whoever is keying up.
    case "tone":
      return null;
  }
}

/** A 20-bit EV1527 address, the five hex digits every remote is quoted by. */
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

/** An unparsable timestamp sorts oldest rather than poisoning the comparator with NaN. */
function timeMs(at: string): number {
  const ms = Date.parse(at);
  return Number.isNaN(ms) ? 0 : ms;
}
