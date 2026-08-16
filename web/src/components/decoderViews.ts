import type { StationOf } from "../lib/decoded";
import type {
  AdsbMessage,
  AisMessage,
  DecodedRecord,
  DecodedRecordOf,
  DvFrame,
  IdentReport,
  Modulation,
  RdsUpdate,
} from "../lib/types";
import { formatHz } from "./format";

export interface DecoderScope {
  deviceSet?: number;
  channel?: number;
}

export function inScope(deviceSet: number, channel: number, scope: DecoderScope): boolean {
  return (
    (scope.deviceSet === undefined || scope.deviceSet === deviceSet) &&
    (scope.channel === undefined || scope.channel === channel)
  );
}

export function recordsInScope<K extends DecodedRecord["event"]["kind"]>(
  records: readonly DecodedRecordOf<K>[],
  scope: DecoderScope,
): readonly DecodedRecordOf<K>[] {
  if (scope.deviceSet === undefined && scope.channel === undefined) {
    return records;
  }
  return records.filter((r) => inScope(r.device_set, r.channel, scope));
}

export function stationsInScope<S extends { deviceSet: number; channel: number }>(
  stations: readonly S[],
  scope: DecoderScope,
): readonly S[] {
  if (scope.deviceSet === undefined && scope.channel === undefined) {
    return stations;
  }
  return stations.filter((s) => inScope(s.deviceSet, s.channel, scope));
}

export const TARGET_STALE_MS = 30_000;

export const TARGET_MAX_AGE_MS = 300_000;

export function ageClass(ageMs: number): string {
  if (ageMs < TARGET_STALE_MS) {
    return "text-ink";
  }
  return ageMs < TARGET_MAX_AGE_MS / 2 ? "text-ink-dim" : "text-ink-dim opacity-50";
}

export function formatAge(ageMs: number): string {
  const seconds = Math.max(0, Math.round(ageMs / 1000));
  if (seconds < 60) {
    return `${seconds}s`;
  }
  const m = Math.floor(seconds / 60);
  return m < 60 ? `${m}:${pad2(seconds % 60)}` : `${Math.floor(m / 60)}h${pad2(m % 60)}`;
}

export interface TargetRow {
  id: string;
  label: string;
  primary: string;
  secondary: string;
  position: string;
  ageMs: number;
  frames: number;
}

export function aircraftRow(station: StationOf<"adsb">, nowMs: number): TargetRow {
  const m: AdsbMessage = station.event.data;
  return {
    id: m.icao.toUpperCase(),
    label: m.callsign?.trim() || "—",
    primary: m.on_ground === true ? "GND" : formatAltitudeFt(m.altitude_ft),
    secondary: joinFields(formatSpeedKt(m.ground_speed_kt), formatBearing(m.track_deg)),
    position: formatPosition(m.lat, m.lon),
    ageMs: Math.max(0, nowMs - station.lastSeen),
    frames: station.frames,
  };
}

export function shipRow(station: StationOf<"ais">, nowMs: number): TargetRow {
  const m: AisMessage = station.event.data;
  return {
    id: String(m.mmsi),
    label: m.name?.trim() || m.call_sign?.trim() || "—",
    primary: formatSpeedKt(m.sog_kt),
    secondary: joinFields(formatBearing(m.cog_deg), m.destination?.trim() ?? ""),
    position: formatPosition(m.lat, m.lon),
    ageMs: Math.max(0, nowMs - station.lastSeen),
    frames: station.frames,
  };
}

export type TargetSort = "age" | "id";

export function sortTargets(
  rows: readonly TargetRow[],
  key: TargetSort,
  descending: boolean,
): TargetRow[] {
  const direction = descending ? -1 : 1;
  return rows.toSorted(
    (a, b) => direction * (key === "age" ? a.ageMs - b.ageMs : compareIds(a.id, b.id)),
  );
}

export function formatAltitudeFt(ft: number | null | undefined): string {
  return ft == null ? "—" : `${groupThousands(Math.round(ft))} ft`;
}

export function formatSpeedKt(kt: number | null | undefined): string {
  return kt == null ? "—" : `${kt.toFixed(0)} kt`;
}

export function formatBearing(deg: number | null | undefined): string {
  if (deg == null) {
    return "";
  }
  return `${((Math.round(deg) % 360) + 360) % 360}°`;
}

export function formatPosition(
  lat: number | null | undefined,
  lon: number | null | undefined,
): string {
  return lat == null || lon == null ? "—" : `${lat.toFixed(5)}, ${lon.toFixed(5)}`;
}

export function formatClock(at: string): string {
  const t = new Date(at);
  if (Number.isNaN(t.getTime())) {
    return "--:--:--";
  }
  return `${pad2(t.getHours())}:${pad2(t.getMinutes())}:${pad2(t.getSeconds())}`;
}

export function rdsPicture(records: readonly DecodedRecordOf<"rds">[]): RdsUpdate | null {
  if (records.length === 0) {
    return null;
  }
  const merged = records.reduceRight<Record<string, unknown>>((acc, r) => {
    for (const [key, value] of Object.entries(r.event.data)) {
      if (value != null) {
        acc[key] = value;
      }
    }
    return acc;
  }, {});
  return merged as unknown as RdsUpdate;
}

export type RdsQualityLabel = "no lock" | "good" | "fair" | "poor";

export interface RdsQuality {
  groups: number;
  blockErrors: number;
  errorRate: number;
  label: RdsQualityLabel;
  className: string;
}

export function rdsQuality(update: RdsUpdate): RdsQuality {
  const groups = update.groups;
  const blockErrors = update.block_errors;
  const blocks = groups * 4 + blockErrors;
  const errorRate = blocks === 0 ? 0 : blockErrors / blocks;
  if (groups === 0) {
    return { groups, blockErrors, errorRate, label: "no lock", className: "text-danger" };
  }
  if (errorRate < 0.02) {
    return { groups, blockErrors, errorRate, label: "good", className: "text-accent" };
  }
  if (errorRate < 0.1) {
    return { groups, blockErrors, errorRate, label: "fair", className: "text-ink" };
  }
  return { groups, blockErrors, errorRate, label: "poor", className: "text-danger" };
}

export function ptyLabel(update: RdsUpdate): string {
  if (update.pty_name != null && update.pty_name !== "") {
    return update.pty_name;
  }
  return update.pty == null ? "—" : `PTY ${update.pty}`;
}

export function formatAltFreqs(hz: readonly number[] | undefined): string[] {
  return (hz ?? []).toSorted((a, b) => a - b).map(formatHz);
}

export const TRANSCRIPT_LIMIT = 20_000;

export function appendTranscript(
  previous: string,
  chunk: string,
  limit = TRANSCRIPT_LIMIT,
): string {
  const joined = previous + chunk;
  if (joined.length <= limit) {
    return joined;
  }
  const cut = joined.slice(joined.length - limit);
  const newline = cut.indexOf("\n");
  return newline === -1 ? cut : cut.slice(newline + 1);
}

export function buildTranscript(
  records: readonly DecodedRecordOf<"rtty" | "morse" | "psk31" | "psk63">[],
  limit = TRANSCRIPT_LIMIT,
): string {
  return records.reduceRight((text, r) => appendTranscript(text, r.event.data.text, limit), "");
}

export const CW_SPOT_GROUP_HZ = 50;

export const CW_SPOT_TEXT_LIMIT = 2_000;

export interface CwSignalRow {
  frequencyHz: number;
  offsetHz: number;
  wpm: number;
  snrDb: number;
  text: string;
}

export function cwSignalRows(
  records: readonly DecodedRecordOf<"cw_skimmer">[],
  limit = CW_SPOT_TEXT_LIMIT,
): CwSignalRow[] {
  const signals = new Map<number, CwSignalRow>();
  for (const record of records.toReversed()) {
    const spot = record.event.data;
    const key = Math.round(spot.offset_hz / CW_SPOT_GROUP_HZ);
    const current = signals.get(key);
    signals.set(key, {
      frequencyHz: record.freq_hz + spot.offset_hz,
      offsetHz: spot.offset_hz,
      wpm: spot.wpm,
      snrDb: spot.snr_db,
      text: `${current?.text ?? ""}${spot.text}`.slice(-limit),
    });
  }
  return [...signals.values()].toSorted((left, right) => left.offsetHz - right.offsetHz);
}

export function latestWpm(records: readonly DecodedRecordOf<"morse">[]): number | null {
  const newest = records[0];
  return newest === undefined ? null : newest.event.data.wpm;
}

export interface ScrollMetrics {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

export function isAtBottom(m: ScrollMetrics, tolerancePx = 8): boolean {
  return m.scrollHeight - m.scrollTop - m.clientHeight <= tolerancePx;
}

export function toneLabel(status: { ctcss_hz?: number | null; dcs_code?: number | null }): string {
  return joinFields(
    status.ctcss_hz == null ? "" : `CTCSS ${status.ctcss_hz.toFixed(1)} Hz`,
    status.dcs_code == null ? "" : `DCS ${String(status.dcs_code).padStart(3, "0")}`,
  );
}

function compareIds(a: string, b: string): number {
  if (a.length !== b.length) {
    return a.length - b.length;
  }
  return a < b ? -1 : a > b ? 1 : 0;
}

function joinFields(...fields: string[]): string {
  return fields.filter((f) => f !== "").join(" · ");
}

function groupThousands(n: number): string {
  const sign = n < 0 ? "−" : "";
  return sign + String(Math.abs(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

const DV_MODE_LABELS: Record<DvFrame["mode"], string> = {
  dmr: "DMR",
  dstar: "D-STAR",
  ysf: "YSF",
  nxdn: "NXDN",
  p25: "P25",
  dpmr: "dPMR",
  m17: "M17",
  freedv: "FreeDV",
};

export function dvMode(frame: Pick<DvFrame, "mode">): string {
  return DV_MODE_LABELS[frame.mode];
}

export function dvNetwork(frame: Pick<DvFrame, "mode" | "color_code" | "slot">): string {
  const parts: string[] = [];
  if (frame.slot != null) {
    parts.push(`TS${frame.slot}`);
  }
  if (frame.color_code != null) {
    if (frame.mode === "p25") {
      parts.push(`NAC ${frame.color_code.toString(16).toUpperCase().padStart(3, "0")}`);
    } else if (frame.mode === "nxdn" || frame.mode === "dpmr") {
      parts.push(`RAN ${frame.color_code}`);
    } else {
      parts.push(`CC ${frame.color_code}`);
    }
  }
  return parts.join(" ");
}

export function dvParties(
  frame: Pick<
    DvFrame,
    "source" | "destination" | "source_call" | "destination_call" | "group_call"
  >,
): string {
  const to =
    frame.destination_call ??
    (frame.destination == null
      ? null
      : frame.group_call === false
        ? String(frame.destination)
        : `TG ${frame.destination}`);
  const from = frame.source_call ?? (frame.source == null ? null : String(frame.source));
  if (to != null && from != null) {
    return `${to} ← ${from}`;
  }
  return to ?? from ?? "";
}

const MODULATION_LABELS: Record<Modulation, string> = {
  none: "no signal",
  carrier: "unmodulated carrier",
  ook: "OOK",
  am: "AM",
  ssb: "SSB",
  fm: "FM",
  fsk2: "2-FSK",
  fsk4: "4-FSK",
  psk2: "BPSK",
  psk4: "QPSK",
  noise_like: "noise-like",
  unknown: "unknown",
};

export function modulationLabel(report: IdentReport): string {
  const base = MODULATION_LABELS[report.modulation] ?? report.modulation;
  return report.sideband == null ? base : `${base} (${report.sideband.toUpperCase()})`;
}

export type IdentField = readonly [label: string, value: string];

export function identMeasurements(report: IdentReport): IdentField[] {
  if (report.modulation === "none") {
    return [["Loudest bin", `${report.snr_db.toFixed(1)} dB over the noise floor`]];
  }
  const fields: IdentField[] = [
    ["Bandwidth", `${(report.bandwidth_hz / 1000).toFixed(1)} kHz`],
    ["Off tune", `${Math.round(report.center_offset_hz)} Hz`],
    ["SNR", `${report.snr_db.toFixed(1)} dB`],
  ];
  if (report.symbol_rate_hz != null) {
    fields.push(["Symbol rate", `${Math.round(report.symbol_rate_hz)} Bd`]);
  }
  if (report.deviation_hz != null) {
    fields.push(["Deviation", `±${Math.round(report.deviation_hz)} Hz`]);
  }
  if (report.features.duty < 0.99) {
    fields.push(["Duty", `${Math.round(report.features.duty * 100)}%`]);
  }
  return fields;
}

export function candidateScore(match: { score: number; confirmed?: boolean }): string {
  return match.confirmed === true ? "confirmed" : `${Math.round(match.score * 100)}%`;
}
