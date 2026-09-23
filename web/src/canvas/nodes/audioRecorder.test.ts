import { describe, expect, it } from "vitest";
import type { AudioRecordingStatus, ChannelInfo } from "../../lib/types";
import { recordingFor } from "./audioRecorder";

const status = (file: string, fx?: string[]): AudioRecordingStatus => ({
  file,
  fx,
  started_at: "2026-09-23T12:00:00Z",
  channels: 1,
  frames: 0,
  bytes: 0,
});

const channel = (audio_recordings?: AudioRecordingStatus[]): ChannelInfo => ({
  id: 1,
  settings: { frequency_hz: 1, params: { type: "nfm", settings: {} } },
  audio_recordings,
});

describe("recordingFor", () => {
  it("finds the recording of the route the recorder is wired through", () => {
    const wired = channel([status("raw.wav"), status("clean.wav", ["a", "b"])]);
    expect(recordingFor(wired, [])?.file).toBe("raw.wav");
    expect(recordingFor(wired, ["a", "b"])?.file).toBe("clean.wav");
  });

  it("is null for a route nobody records, even through the same nodes in another order", () => {
    const wired = channel([status("clean.wav", ["a", "b"])]);
    expect(recordingFor(wired, ["b", "a"])).toBeNull();
    expect(recordingFor(wired, [])).toBeNull();
    expect(recordingFor(channel(), [])).toBeNull();
  });
});
