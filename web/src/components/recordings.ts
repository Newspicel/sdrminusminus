import type { DeviceSet, RecordingFormat, RecordingInfo, RecordingStatus } from "../lib/types";
import { formatMhz } from "./format";

export const MAX_RECORDING_TAGS = 32;
export const MAX_RECORDING_TAG_LEN = 48;

export const downloadFormats: ReadonlyArray<{
  format: RecordingFormat;
  label: string;
  hint: string;
}> = [
  {
    format: "sigmf",
    label: ".sigmf",
    hint: "SigMF archive — metadata and samples, exactly as recorded",
  },
  {
    format: "wav",
    label: ".wav",
    hint: "I/Q as a float WAV for HDSDR, SDR# or Audacity — keeps the samples, but only the center frequency and start time of the metadata",
  },
];

export type RecordControl =
  | { kind: "idle"; canStart: boolean }
  | { kind: "recording"; status: RecordingStatus };

export function deriveRecordControl(set: DeviceSet): RecordControl {
  if (set.recording != null) {
    return { kind: "recording", status: set.recording };
  }
  return { kind: "idle", canStart: set.status === "running" };
}

export function recordingElapsedS(
  status: RecordingStatus,
  nowMs: number,
  sampleRate: number,
): number {
  if (status.error != null) {
    return sampleRate > 0 ? status.samples / sampleRate : 0;
  }
  const started = Date.parse(status.started_at);
  return Number.isNaN(started) ? 0 : Math.max(0, (nowMs - started) / 1000);
}

export function parseTags(input: string): string[] {
  const tags: string[] = [];
  for (const raw of input.split(",")) {
    const tag = raw.trim().slice(0, MAX_RECORDING_TAG_LEN);
    if (tag !== "" && !tags.some((kept) => kept.toLowerCase() === tag.toLowerCase())) {
      tags.push(tag);
    }
  }
  return tags.slice(0, MAX_RECORDING_TAGS);
}

export function formatTags(tags: readonly string[]): string {
  return tags.join(", ");
}

export function matchesRecordingSearch(recording: RecordingInfo, search: string): boolean {
  const needle = search.trim().toLowerCase();
  if (needle === "") {
    return true;
  }
  const haystack = [recording.file, recording.note ?? "", ...(recording.tags ?? [])];
  return haystack.some((field) => field.toLowerCase().includes(needle));
}

export function formatDuration(seconds: number): string {
  const tenths = Math.round(seconds * 10) / 10;
  if (tenths < 60) {
    return `${tenths.toFixed(1)} s`;
  }
  const whole = Math.round(tenths);
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  return h > 0 ? `${h}:${pad2(m)}:${pad2(s)}` : `${m}:${pad2(s)}`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1e3) {
    return `${bytes} B`;
  }
  if (bytes < 1e6) {
    return `${(bytes / 1e3).toFixed(1)} kB`;
  }
  if (bytes < 1e9) {
    return `${(bytes / 1e6).toFixed(1)} MB`;
  }
  return `${(bytes / 1e9).toFixed(2)} GB`;
}

export function formatRecordedAt(createdAt: string): string | null {
  const at = Date.parse(createdAt);
  return Number.isNaN(at)
    ? null
    : new Date(at).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
}

export function describeRecording(recording: RecordingInfo): string {
  return [
    formatMhz(recording.center_hz),
    `${(recording.sample_rate / 1e6).toFixed(3)} MS/s`,
    formatDuration(recording.duration_s),
    formatBytes(recording.bytes),
  ].join(" · ");
}

export function recordingProvenance(recording: RecordingInfo): string {
  const when = formatRecordedAt(recording.created_at);
  return [
    ...(when === null ? [] : [when]),
    recording.device_label,
    ...(recording.tags ?? []).map((tag) => `#${tag}`),
  ].join(" · ");
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}
