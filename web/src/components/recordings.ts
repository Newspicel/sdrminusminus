import type { DeviceSet, RecordingFormat, RecordingStatus } from "../lib/types";

// What each container costs the operator, said once so the panel and the canvas agree. The
// archive is the one that survives a round trip back into sdr--; the WAV is for other tools.
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

// A faulted recording still reads as "recording": the writer has already stopped, but only an
// explicit stop clears the surfaced error and frees the set for the next start ( — the
// fault must stay visible, not vanish on the next state refresh).
export function deriveRecordControl(set: DeviceSet): RecordControl {
  if (set.recording != null) {
    return { kind: "recording", status: set.recording };
  }
  return { kind: "idle", canStart: set.status === "running" };
}

// Wall-clock elapsed while healthy; a faulted recording's writer has already stopped, so the
// readout freezes at the captured duration (samples / rate) instead of counting dead air.
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

export function formatDuration(seconds: number): string {
  // Round to tenths first so 59.99 lands in the m:ss branch, not as "60.0 s".
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

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}
