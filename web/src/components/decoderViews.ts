// Pure view logic for the live decoder panes (PLAN §13). Everything a test can pin down —
// row projection, ageing, sorting, the transcript trim rule, filters — lives here so the
// components in `DecoderPanels.tsx` stay render-only.
import type { StationOf } from "../lib/decoded";
import type {
  AdsbMessage,
  AisMessage,
  DecodedRecord,
  DecodedRecordOf,
  DvFrame,
  RdsUpdate,
} from "../lib/types";
import { formatHz } from "./format";

/** Which device set / channel a view is showing. An absent field matches everything, so a view
 * placed before a channel is selected still shows the decoder's traffic. */
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

// ── ageing ────────────────────────────────────────────────────────────────────────────────

/** A target that has not been heard for this long is no longer "live" — it is dimmed rather
 * than removed, because silence is the only thing a transmitter out of range reports. */
export const TARGET_STALE_MS = 30_000;

/** Horizon the views hand to `ageOut`: past this a target disappears entirely. */
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

// ── target rows ───────────────────────────────────────────────────────────────────────────

export interface TargetRow {
  id: string;
  /** Callsign (ADS-B) or vessel name (AIS); `—` until a frame carrying it arrives. */
  label: string;
  /** Altitude for aircraft, speed over ground for ships. */
  primary: string;
  /** Speed + track for aircraft, course + destination for ships. */
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

/** Ascending age is "most recently heard first" — the useful default for a live target list. */
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

// ── formatting ────────────────────────────────────────────────────────────────────────────

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

/** Five decimals ≈ 1 m — enough to be useful, short enough for a phone column. */
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

// ── RDS ───────────────────────────────────────────────────────────────────────────────────

/** Folds the scoped frames into one picture. The store merges forward per PI, but a transmitter
 * whose PI has not been received yet has no station row at all, and that is exactly when the
 * operator is staring at the panel — so the view folds the frames itself. */
export function rdsPicture(records: readonly DecodedRecordOf<"rds">[]): RdsUpdate | null {
  if (records.length === 0) {
    return null;
  }
  // Records arrive newest-first, so folding from the tail lets a newer non-null field win.
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
  /** Rejected blocks over all blocks seen; each accepted group is four blocks. */
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

// ── rolling text (RTTY / Morse) ───────────────────────────────────────────────────────────

/** Characters kept in a transcript pane. Beyond this the head is dropped — the pane is a live
 * tail; the stored history is `GET /api/decoderlog`. */
export const TRANSCRIPT_LIMIT = 20_000;

/** Trims from the head at a line boundary when there is one, so the top of the pane is never a
 * half-eaten line. */
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

/** Builds the pane's text from the store's newest-first frames. Deriving it beats accumulating
 * local state: a cleared or re-flushed store can never leave the pane showing a stale tail. */
export function buildTranscript(
  records: readonly DecodedRecordOf<"rtty" | "morse">[],
  limit = TRANSCRIPT_LIMIT,
): string {
  return records.reduceRight((text, r) => appendTranscript(text, r.event.data.text, limit), "");
}

/** WPM of the most recent Morse frame — the tracked sending speed, not an average over history. */
export function latestWpm(records: readonly DecodedRecordOf<"morse">[]): number | null {
  const newest = records[0];
  return newest === undefined ? null : newest.event.data.wpm;
}

export interface ScrollMetrics {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

/** Sub-pixel scroll heights and zoom make an exact comparison flaky, hence the tolerance. */
export function isAtBottom(m: ScrollMetrics, tolerancePx = 8): boolean {
  return m.scrollHeight - m.scrollTop - m.clientHeight <= tolerancePx;
}

// ── POCSAG ────────────────────────────────────────────────────────────────────────────────

/** RICs are quoted as 7 digits, zero padded, so a column of them stays aligned. */
export function formatRic(address: number): string {
  return String(address).padStart(7, "0");
}

const FUNCTION_LABELS = ["A", "B", "C", "D"];

/** Function bits 0–3 are what a pager labels A–D. */
export function functionLabel(fn: number): string {
  return FUNCTION_LABELS[fn] ?? String(fn);
}

export function matchesAddress(address: number, filter: string): boolean {
  const digits = filter.replace(/\D/g, "");
  return digits === "" || formatRic(address).includes(digits);
}

// ── APRS ──────────────────────────────────────────────────────────────────────────────────

/** The Mic-E message and course/speed/altitude as one trailing line; empty when the packet
 * carried none of them. The message leads because it is what the operator chose to say —
 * "Emergency" is the one thing on this line that is not a measurement. */
export function aprsMotion(packet: {
  course_deg?: number | null;
  speed_kt?: number | null;
  altitude_ft?: number | null;
  mic_e_message?: string | null;
}): string {
  return joinFields(
    packet.mic_e_message ?? "",
    formatBearing(packet.course_deg),
    packet.speed_kt == null ? "" : formatSpeedKt(packet.speed_kt),
    packet.altitude_ft == null ? "" : formatAltitudeFt(packet.altitude_ft),
  );
}

// ── subaudible signalling ─────────────────────────────────────────────────────────────────

/** What is under the carrier, named the way a radio names it: a CTCSS tone in Hz to one
 * decimal, a DCS code as its three octal digits. Empty when there is nothing under it. */
export function toneLabel(status: { ctcss_hz?: number | null; dcs_code?: number | null }): string {
  return joinFields(
    status.ctcss_hz == null ? "" : `CTCSS ${status.ctcss_hz.toFixed(1)} Hz`,
    status.dcs_code == null ? "" : `DCS ${String(status.dcs_code).padStart(3, "0")}`,
  );
}

// ── helpers ───────────────────────────────────────────────────────────────────────────────

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

// ── NAVTEX ────────────────────────────────────────────────────────────────────────────────

/** The `B1B2B3B4` group as broadcast, or `null` when the header was missed — a receiver that
 * joined mid-broadcast still has the text, and hiding it behind a missing header would be worse
 * than showing a message with no serial. */
export function navtexHeader(message: {
  station?: string | null;
  subject?: string | null;
  serial?: number | null;
}): string | null {
  const { station, subject, serial } = message;
  if (station == null || subject == null || serial == null) {
    return null;
  }
  return `${station}${subject}${String(serial).padStart(2, "0")}`;
}

/** The provenance line under a broadcast: what the FEC repaired, and whether `NNNN` ever came. */
export function navtexQuality(message: { errors_corrected: number; complete: boolean }): string {
  const parts: string[] = [];
  if (!message.complete) {
    parts.push("truncated");
  }
  if (message.errors_corrected > 0) {
    parts.push(`${message.errors_corrected} repaired`);
  }
  return parts.join(" · ");
}

// ── ACARS ─────────────────────────────────────────────────────────────────────────────────

/** Who sent it: registration, and the flight number when the block carries one. */
export function acarsHeadline(message: { registration: string; flight?: string | null }): string {
  return joinFields(message.registration, message.flight?.trim() ?? "");
}

/** The block's routing fields as one compact tag — label, block id, direction, ack. */
export function acarsTag(message: {
  label: string;
  block_id: string;
  downlink: boolean;
  ack?: string | null;
  more: boolean;
}): string {
  return joinFields(
    message.label,
    message.downlink ? "DL" : "UL",
    message.ack == null ? "NAK" : "",
    message.more ? "more" : "",
  );
}

// ── sub-GHz ───────────────────────────────────────────────────────────────────────────────

/** What the burst turned out to be: the payload, or the size of the raw capture. */
export function subghzPayload(frame: {
  bits: number;
  data: string;
  timings_us?: number[];
}): string {
  return frame.bits === 0
    ? `raw, ${(frame.timings_us ?? []).length} edges`
    : `${frame.data} (${frame.bits} bit)`;
}

/** The device readings a 24-bit payload supports, when it supports them. Empty for anything the
 * classifier could not name — which is honest, not a gap. */
export function subghzReadings(frame: {
  address?: number | null;
  button?: number | null;
  tri_state?: string | null;
}): string {
  return joinFields(
    frame.address == null
      ? ""
      : `addr ${frame.address.toString(16).toUpperCase().padStart(5, "0")}`,
    frame.button == null ? "" : `btn ${frame.button.toString(16).toUpperCase()}`,
    frame.tri_state == null ? "" : `PT ${frame.tri_state}`,
  );
}

/** Base period and repeat count — the two numbers that say whether a decode should be trusted. */
export function subghzTiming(frame: { short_us: number; repeats: number }): string {
  return joinFields(
    frame.short_us > 0 ? `${frame.short_us} µs` : "",
    frame.repeats > 1 ? `×${frame.repeats}` : "",
  );
}

// ── digital voice ─────────────────────────────────────────────────────────────────────────

/** Mode names as operators write them, which is not how the wire spells them. */
const DV_MODE_LABELS: Record<DvFrame["mode"], string> = {
  dmr: "DMR",
  dstar: "D-STAR",
  ysf: "YSF",
  nxdn: "NXDN",
  p25: "P25",
  dpmr: "dPMR",
  m17: "M17",
};

export function dvMode(frame: Pick<DvFrame, "mode">): string {
  return DV_MODE_LABELS[frame.mode];
}

/** The network the frame was heard on, under whichever name its mode publishes: a DMR colour
 * code, an NXDN or dPMR RAN, a P25 network access code (which everyone quotes in hex). */
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

/** Who is talking to whom, by whichever name the mode addresses them. `TG` marks a talkgroup,
 * because a bare number next to a radio ID would read as another radio. */
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

/** What the frame was: the words a scanner shows rather than the specification's field names. */
export function dvKind(frame: Pick<DvFrame, "kind">): string {
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
