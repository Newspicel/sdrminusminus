// Replay transport arithmetic, kept out of the component so the parts that can be wrong —
// clamping, the empty recording, the clock — are testable without a DOM.
import type { DeviceSet, PlaybackStatus } from "../lib/types";

// The `loop` device setting a playback backend declares (`LOOP_SETTING` in
// `crates/device-virtual/src/playback.rs`). Extras are a per-device list rather than a wire
// struct, so this name is not in the generated schema: it is the one string the transport has
// to know, and the device's own `apply` rejects it if the two ever drift.
export const LOOP_SETTING = "loop";

/** 0..1 through the recording. An empty one reads as 0, never as a NaN-wide progress bar. */
export function playbackProgress(status: PlaybackStatus): number {
  if (status.total_samples <= 0) {
    return 0;
  }
  return Math.min(1, status.position_samples / status.total_samples);
}

/**
 * Where playback has reached `elapsedMs` after the snapshot that reported `status`.
 *
 * The server publishes a position only when something emits a state change, so between those
 * the bar has to advance on the clock. Without this it sits frozen and then jumps forward the
 * moment anything else touches the set — which reads as the recording skipping, when all that
 * moved was the readout catching up.
 */
export function playbackPositionAt(
  status: PlaybackStatus,
  elapsedMs: number,
  sampleRate: number,
  looping: boolean,
): number {
  const reported = Math.min(status.position_samples, status.total_samples);
  if (status.paused || sampleRate <= 0 || status.total_samples <= 0) {
    return reported;
  }
  const advanced = reported + Math.max(0, elapsedMs / 1000) * sampleRate;
  if (advanced < status.total_samples) {
    return advanced;
  }
  // Past the end: wrap with the loop, or hold at the end, exactly as the worker does.
  return looping ? advanced % status.total_samples : status.total_samples;
}

/** Samples as seconds at the set's rate; 0 for a rate we do not have, so the clock reads
 * `0:00` rather than `NaN:aN`. */
export function samplesToSeconds(samples: number, sampleRate: number): number {
  return sampleRate > 0 ? samples / sampleRate : 0;
}

/** Transport clock, always `m:ss` (or `h:mm:ss`). Deliberately not the library's
 * `formatDuration`, which reads in tenths under a minute: a readout that changes width every
 * frame is unreadable while it runs. */
export function formatClock(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  const pad = (n: number): string => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** Whether this set replays a recording on a loop. Absent means the backend's default of on,
 * matching `ExtraSetting::Bool { default: true }`. */
export function isLooping(set: DeviceSet): boolean {
  const value = set.settings.extra?.find((extra) => extra.name === LOOP_SETTING)?.value;
  return typeof value === "boolean" ? value : true;
}
