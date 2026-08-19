import type { DeviceSet, PlaybackStatus } from "../lib/types";

export const LOOP_SETTING = "loop";

export function playbackProgress(status: PlaybackStatus): number {
  if (status.total_samples <= 0) {
    return 0;
  }
  return Math.min(1, status.position_samples / status.total_samples);
}

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
  return looping ? advanced % status.total_samples : status.total_samples;
}

export function samplesToSeconds(samples: number, sampleRate: number): number {
  return sampleRate > 0 ? samples / sampleRate : 0;
}

const pad = (n: number): string => String(n).padStart(2, "0");

export function formatClock(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

export function isLooping(set: DeviceSet): boolean {
  const value = set.settings.extra?.find((extra) => extra.name === LOOP_SETTING)?.value;
  return typeof value === "boolean" ? value : true;
}
