import { passesChain } from "../canvas/nodes/eventFilter";
import type { DecodedState } from "../lib/decoded";
import type {
  DecodedRecord,
  DecoderEvent,
  DecoderKind,
  DecoderLogEntry,
  DecoderLogFilter,
  EventFilterNode,
} from "../lib/types";
import { eventStation, eventSummary, hasPosition, hex2, hex5 } from "./eventFacts";

export { eventStation, eventSummary, hasPosition, hex2, hex5 };

export const KIND_LABELS: Record<DecoderKind, string> = {
  call: "Call",
  adsb: "ADS-B",
  ais: "AIS",
  aprs: "APRS",
  pocsag: "POCSAG",
  flex: "FLEX",
  ermes: "ERMES",
  rds: "RDS",
  rtty: "RTTY",
  morse: "Morse",
  cw_skimmer: "CW skimmer",
  selcall: "Selcall",
  navtex: "NAVTEX",
  acars: "ACARS",
  subghz: "Sub-GHz",
  tone: "Tone",
  scrambler: "Scrambler",
  dv: "Digital voice",
  ident: "Signal ID",
  ft8: "FT8",
  ft4: "FT4",
  psk: "PSK",
  wspr: "WSPR",
  broadcast: "Digital broadcast",
  radio_clock: "Radio clock",
  gnss: "GNSS lab",
  sstv: "SSTV",
  vor: "VOR",
  ils: "ILS",
  dsc: "DSC",
  inmarsat_stdc: "Inmarsat STD-C",
  inmarsat_aero: "Inmarsat Aero",
  vdl2: "VDL Mode 2",
  hfdl: "HFDL",
  iridium: "Iridium",
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

export const LOG_COLUMNS = [
  { key: "at", label: "Time", width: 84 },
  { key: "kind", label: "Kind", width: 96 },
  { key: "freq", label: "Frequency", width: 116 },
  { key: "station", label: "Station", width: 152 },
  { key: "summary", label: "Summary", width: 320 },
] as const;

export type LogColumnKey = (typeof LOG_COLUMNS)[number]["key"];

export type ColumnWidths = Record<LogColumnKey, number>;

export const MIN_COLUMN_WIDTH = 56;

export const MAX_COLUMN_WIDTH = 720;

export const COLUMN_STEP = 8;

const COLUMN_KEY = "sdrmm.decoderLog.columns";

export function defaultColumnWidths(): ColumnWidths {
  const widths = {} as ColumnWidths;
  for (const column of LOG_COLUMNS) {
    widths[column.key] = column.width;
  }
  return widths;
}

export function clampColumnWidth(px: number): number {
  if (!Number.isFinite(px)) {
    return MIN_COLUMN_WIDTH;
  }
  return Math.round(Math.min(MAX_COLUMN_WIDTH, Math.max(MIN_COLUMN_WIDTH, px)));
}

export function resizeColumn(widths: ColumnWidths, key: LogColumnKey, px: number): ColumnWidths {
  return { ...widths, [key]: clampColumnWidth(px) };
}

export function totalColumnWidth(widths: ColumnWidths): number {
  return LOG_COLUMNS.reduce((sum, column) => sum + widths[column.key], 0);
}

function storage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export function readColumnWidths(): ColumnWidths {
  const widths = defaultColumnWidths();
  let stored: unknown;
  try {
    const raw = storage()?.getItem(COLUMN_KEY);
    stored = raw === null || raw === undefined ? null : JSON.parse(raw);
  } catch {
    return widths;
  }
  if (stored === null || typeof stored !== "object") {
    return widths;
  }
  const record = stored as Record<string, unknown>;
  for (const column of LOG_COLUMNS) {
    const value = record[column.key];
    if (typeof value === "number" && Number.isFinite(value)) {
      widths[column.key] = clampColumnWidth(value);
    }
  }
  return widths;
}

export function writeColumnWidths(widths: ColumnWidths): void {
  try {
    storage()?.setItem(COLUMN_KEY, JSON.stringify(widths));
  } catch {}
}

export function kindLabel(kind: string): string {
  return KIND_LABELS[kind as DecoderKind] ?? kind.toUpperCase();
}

export interface WireScope {
  nodes: string;
  sources: string;
  gate: EventGate;
}

export interface EventGate {
  kinds: string[];
  bySource: Record<string, EventFilterNode[][]>;
}

export const NO_GATE: EventGate = { kinds: [], bySource: {} };

export const NO_WIRES: WireScope = { nodes: "", sources: "", gate: NO_GATE };

export function passesGate(gate: EventGate, source: string, event: DecoderEvent): boolean {
  const chains = gate.bySource[source];
  if (chains === undefined || chains.length === 0) {
    return true;
  }
  return chains.some((chain) => passesChain(chain, event));
}

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
  if (wires.gate.kinds.length > 0) {
    query.kinds = wires.gate.kinds.join(",");
  }
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
  gate: EventGate = NO_GATE,
): boolean {
  if (!inSources(record, sources)) {
    return false;
  }
  if (!passesGate(gate, `${record.device_set}:${record.channel}`, record.event)) {
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
  gate: EventGate = NO_GATE,
  cap = LIVE_ROW_CAP,
): DecodedRecord[] {
  const records: DecodedRecord[] = [];
  for (const slice of Object.values(frames)) {
    for (const record of slice ?? []) {
      if (matchesFilter(record, filter, sources, gate)) {
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

function timeMs(at: string): number {
  const ms = Date.parse(at);
  return Number.isNaN(ms) ? 0 : ms;
}
