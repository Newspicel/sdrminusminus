import type { AudioRecordingStatus, ChannelInfo } from "../../lib/types";

function sameRoute(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((node, index) => node === b[index]);
}

export function recordingFor(
  channel: ChannelInfo,
  fx: readonly string[],
): AudioRecordingStatus | null {
  return (channel.audio_recordings ?? []).find((status) => sameRoute(status.fx ?? [], fx)) ?? null;
}
